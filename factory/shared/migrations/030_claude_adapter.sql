BEGIN;

ALTER TABLE craft_queue.work ADD CONSTRAINT supported_agent_adapter CHECK (
    NOT (payload ? 'agent_adapter') OR COALESCE(
        payload->'agent_adapter' IN ('"codex"'::jsonb, '"claude"'::jsonb)
        AND (payload->>'agent_adapter' <> 'claude' OR payload->>'execution_profile' IS NULL), false)
);

CREATE FUNCTION craft_queue.claim(
    runner text, request uuid, context_capable boolean, profile_capable boolean, available_repositories text[], selected_adapter text
)
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
    mode text;
BEGIN
    IF COALESCE(selected_adapter, '') NOT IN ('codex', 'claude') THEN RETURN; END IF;
    SELECT * INTO STRICT settings FROM craft_queue.control FOR UPDATE;
    SELECT c.mode INTO mode FROM craft_queue.runner_controls AS c
    WHERE c.runner_id = runner ORDER BY c.revision DESC LIMIT 1;
    IF mode = 'stopped' THEN RETURN; END IF;
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
          AND COALESCE(w.payload->>'agent_adapter', 'codex') = selected_adapter
          AND (available_repositories IS NULL OR w.repository = ANY(available_repositories))
          AND craft_queue.standing_runner_allowed(w.id, runner, now_at)
          AND craft_queue.series_blocker(w.id) IS NULL
          AND (profile_capable OR w.payload->>'execution_profile' IS NULL)
          AND (context_capable OR NOT EXISTS (SELECT 1 FROM craft_queue.project_context_pins(w.payload)));
        RETURN;
    END IF;
    IF settings.paused OR mode = 'draining' THEN RETURN; END IF;

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
      AND COALESCE(w.payload->>'agent_adapter', 'codex') = selected_adapter
      AND (available_repositories IS NULL OR w.repository = ANY(available_repositories))
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

CREATE OR REPLACE FUNCTION craft_queue.claim(
    runner text, request uuid, context_capable boolean, profile_capable boolean, available_repositories text[]
) RETURNS TABLE (work_id text, repository text, generation bigint, expires_at timestamptz, work_order jsonb)
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT * FROM craft_queue.claim(runner, request, context_capable, profile_capable, available_repositories, 'codex');
$$;

REVOKE ALL ON FUNCTION craft_queue.claim(text, uuid, boolean, boolean, text[], text) FROM PUBLIC;
COMMIT;
