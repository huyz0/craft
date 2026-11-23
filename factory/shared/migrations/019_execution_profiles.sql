BEGIN;

CREATE FUNCTION craft_queue.execution_profile_valid(profile jsonb) RETURNS boolean
LANGUAGE sql IMMUTABLE SET search_path = pg_catalog AS $$
    SELECT CASE
        WHEN profile IS NULL OR profile = 'null'::jsonb THEN true
        WHEN jsonb_typeof(profile) IS DISTINCT FROM 'object' THEN false
        ELSE COALESCE(
            profile - ARRAY['id', 'model', 'reasoning_effort', 'timeout_seconds', 'max_attempts',
                'approval_reference'] = '{}'::jsonb
            AND jsonb_typeof(profile->'id') = 'string'
            AND (profile->>'id') COLLATE "C" ~ '^[A-Za-z0-9][A-Za-z0-9_.-]{0,99}$'
            AND jsonb_typeof(profile->'model') = 'string'
            AND (profile->>'model') COLLATE "C" ~ '^[A-Za-z0-9][A-Za-z0-9_.-]{0,99}$'
            AND jsonb_typeof(profile->'reasoning_effort') = 'string'
            AND profile->>'reasoning_effort' IN ('low', 'medium', 'high', 'xhigh', 'max', 'ultra')
            AND jsonb_typeof(profile->'approval_reference') = 'string'
            AND octet_length(profile->>'approval_reference') BETWEEN 1 AND 500
            AND profile->>'approval_reference' ~ '[^[:space:]]'
            AND profile->>'approval_reference' !~ '[[:cntrl:]]'
            AND jsonb_typeof(profile->'timeout_seconds') = 'number'
            AND CASE WHEN profile->>'timeout_seconds' ~ '^[1-9][0-9]{0,3}$'
                THEN (profile->>'timeout_seconds')::integer <= 3600 ELSE false END
            AND jsonb_typeof(profile->'max_attempts') = 'number'
            AND profile->>'max_attempts' ~ '^([1-9]|1[0-2])$', false)
    END;
$$;

ALTER TABLE craft_queue.work ADD CONSTRAINT valid_execution_profile
CHECK (craft_queue.execution_profile_valid(payload->'execution_profile'));

ALTER TABLE craft_queue.attempts ADD COLUMN profile_started_at timestamptz;

CREATE FUNCTION craft_queue.start_profiled_attempt(runner text, lease_work_id text, lease_generation bigint)
RETURNS timestamptz
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    now_at timestamptz;
    started timestamptz;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    now_at := clock_timestamp();
    UPDATE craft_queue.attempts AS a SET profile_started_at = now_at
    FROM craft_queue.work AS w, craft_queue.runner_repositories AS grant_row
    WHERE w.id = lease_work_id AND w.generation = lease_generation AND w.state = 'leased'
      AND w.payload->>'execution_profile' IS NOT NULL
      AND a.work_id = w.id AND a.generation = w.generation AND a.runner_id = runner
      AND a.outcome = 'leased' AND a.expires_at > now_at AND a.profile_started_at IS NULL
      AND grant_row.runner_id = runner AND grant_row.repository = w.repository
      AND craft_queue.series_blocker(w.id) IS NULL
    RETURNING a.profile_started_at INTO started;
    RETURN started;
END;
$$;

CREATE FUNCTION craft_queue.claim(runner text, request uuid, context_capable boolean, profile_capable boolean)
RETURNS TABLE (work_id text, repository text, generation bigint, expires_at timestamptz, work_order jsonb)
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    settings craft_queue.control%ROWTYPE;
    selected craft_queue.work%ROWTYPE;
    previous craft_queue.attempts%ROWTYPE;
    now_at timestamptz;
    review_free boolean;
    tokens_free numeric;
    deadline timestamptz;
BEGIN
    SELECT * INTO STRICT settings FROM craft_queue.control FOR UPDATE;
    now_at := clock_timestamp();
    SELECT * INTO previous FROM craft_queue.attempts AS a
    WHERE a.runner_id = runner AND a.request_id = request;
    IF FOUND THEN
        RETURN QUERY SELECT w.id, w.repository, w.generation, previous.expires_at,
            (w.payload - 'repository') || CASE WHEN w.repair_from_generation IS NULL
                THEN '{}'::jsonb ELSE '{"shared_repair":true}'::jsonb END
        FROM craft_queue.work AS w
        JOIN craft_queue.runner_repositories AS grant_row ON grant_row.repository = w.repository
        WHERE w.id = previous.work_id AND w.generation = previous.generation
          AND w.state = 'leased' AND previous.outcome = 'leased' AND previous.expires_at > now_at
          AND grant_row.runner_id = runner
          AND craft_queue.standing_runner_allowed(w.id, runner, now_at)
          AND craft_queue.series_blocker(w.id) IS NULL
          AND (profile_capable OR w.payload->>'execution_profile' IS NULL)
          AND (context_capable OR NOT EXISTS (SELECT 1 FROM craft_queue.project_context_pins(w.payload)));
        RETURN;
    END IF;
    IF settings.paused THEN RETURN; END IF;

    SELECT COUNT(*) < settings.review_limit INTO review_free
    FROM craft_queue.work AS w
    LEFT JOIN craft_queue.standing_work AS standing ON standing.work_id = w.id
    WHERE w.state IN ('leased', 'candidate')
      OR (w.state = 'queued' AND (w.repair_from_generation IS NOT NULL OR standing.work_id IS NOT NULL));
    SELECT settings.token_budget - COALESCE(SUM(COALESCE(a.reported_tokens, a.token_limit)), 0)
    INTO tokens_free FROM craft_queue.attempts AS a
    WHERE a.started_at > now_at - interval '24 hours'
       OR (a.outcome = 'leased' AND a.expires_at > now_at);

    SELECT w.* INTO selected
    FROM craft_queue.work AS w
    JOIN craft_queue.runner_repositories AS grant_row ON grant_row.repository = w.repository
    LEFT JOIN craft_queue.attempts AS a ON a.work_id = w.id AND a.generation = w.generation
    LEFT JOIN craft_queue.standing_work AS standing ON standing.work_id = w.id
    WHERE grant_row.runner_id = runner AND w.reserved_tokens <= tokens_free
      AND craft_queue.standing_runner_allowed(w.id, runner, now_at)
      AND craft_queue.series_blocker(w.id) IS NULL
      AND (w.payload->>'execution_profile' IS NULL OR (profile_capable
        AND w.generation < (w.payload->'execution_profile'->>'max_attempts')::bigint))
      AND (context_capable OR NOT EXISTS (SELECT 1 FROM craft_queue.project_context_pins(w.payload)))
      AND ((w.state = 'queued' AND (review_free OR w.repair_from_generation IS NOT NULL OR standing.work_id IS NOT NULL))
        OR (w.state = 'leased' AND a.outcome = 'leased' AND a.expires_at <= now_at))
    ORDER BY (w.state = 'leased' OR w.repair_from_generation IS NOT NULL OR standing.work_id IS NOT NULL) DESC,
        w.created_at, w.id
    LIMIT 1;
    IF NOT FOUND THEN RETURN; END IF;

    UPDATE craft_queue.attempts AS a SET outcome = 'expired'
    WHERE a.work_id = selected.id AND a.generation = selected.generation AND a.outcome = 'leased';
    UPDATE craft_queue.work AS w SET state = 'leased', generation = w.generation + 1
    WHERE w.id = selected.id RETURNING w.* INTO selected;
    deadline := now_at + make_interval(secs => settings.lease_seconds);
    INSERT INTO craft_queue.attempts (work_id, generation, runner_id, request_id, expires_at, started_at, token_limit)
    VALUES (selected.id, selected.generation, runner, request, deadline, now_at, selected.reserved_tokens);
    RETURN QUERY SELECT selected.id, selected.repository, selected.generation, deadline,
        (selected.payload - 'repository') || CASE WHEN selected.repair_from_generation IS NULL
            THEN '{}'::jsonb ELSE '{"shared_repair":true}'::jsonb END;
END;
$$;

CREATE OR REPLACE FUNCTION craft_queue.claim(runner text, request uuid, context_capable boolean)
RETURNS TABLE (work_id text, repository text, generation bigint, expires_at timestamptz, work_order jsonb)
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT * FROM craft_queue.claim(runner, request, context_capable, false);
$$;

CREATE OR REPLACE FUNCTION craft_queue.status(work_id text) RETURNS jsonb
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT craft_queue.status_before_standing($1) ||
        CASE WHEN binding.work_id IS NOT NULL
            THEN jsonb_build_object('standing_delegation', to_jsonb(binding) - 'work_id')
            ELSE '{}'::jsonb END ||
        CASE WHEN w.payload->>'execution_profile' IS NOT NULL THEN jsonb_build_object(
            'execution_profile', w.payload->'execution_profile',
            'profile_started_at', a.profile_started_at,
            'attempt_limit_reached',
                w.generation >= (w.payload->'execution_profile'->>'max_attempts')::bigint,
            'attempt_limit_blocks_pickup',
                w.generation >= (w.payload->'execution_profile'->>'max_attempts')::bigint
                AND (w.state = 'queued' OR (w.state = 'leased' AND a.expires_at <= statement_timestamp()))
        ) ELSE '{}'::jsonb END
    FROM craft_queue.work AS w
    LEFT JOIN craft_queue.standing_work AS binding ON binding.work_id = w.id
    LEFT JOIN craft_queue.attempts AS a ON a.work_id = w.id AND a.generation = w.generation
    WHERE w.id = $1;
$$;

REVOKE ALL ON FUNCTION craft_queue.execution_profile_valid(jsonb) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.start_profiled_attempt(text, text, bigint) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.claim(text, uuid, boolean, boolean) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.status(text) FROM PUBLIC;
COMMIT;
