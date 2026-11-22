BEGIN;

CREATE TABLE craft_queue.runner_controls (
    runner_id text NOT NULL,
    revision bigint NOT NULL CHECK (revision > 0),
    mode text NOT NULL CHECK (mode IN ('active', 'draining', 'stopped')),
    actor text NOT NULL,
    evidence text NOT NULL,
    recorded_at timestamptz NOT NULL,
    fenced_leases bigint NOT NULL CHECK (fenced_leases >= 0),
    PRIMARY KEY (runner_id, revision)
);
CREATE INDEX attempts_leased_by_runner ON craft_queue.attempts (runner_id, expires_at) WHERE outcome = 'leased';

CREATE FUNCTION craft_queue.runner_status(runner text) RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT jsonb_build_object(
        'runner_id', runner,
        'mode', COALESCE(latest.mode, 'active'),
        'revision', COALESCE(latest.revision, 0),
        'decision', to_jsonb(latest),
        'live_leases', (SELECT count(*) FROM craft_queue.attempts AS a
            JOIN craft_queue.work AS w ON w.id = a.work_id AND w.generation = a.generation
            WHERE a.runner_id = runner AND w.state = 'leased' AND a.outcome = 'leased'
                AND a.expires_at > statement_timestamp())
    )
    FROM (SELECT 1) AS seed
    LEFT JOIN LATERAL (
        SELECT c.* FROM craft_queue.runner_controls AS c WHERE c.runner_id = runner
        ORDER BY c.revision DESC LIMIT 1
    ) AS latest ON true
    WHERE latest.revision IS NOT NULL OR EXISTS (
        SELECT 1 FROM craft_queue.runner_repositories AS e WHERE e.runner_id = runner
    );
$$;

CREATE FUNCTION craft_queue.set_runner_mode(
    actor_id text, runner text, expected_revision bigint, desired_mode text, evidence_ref text
) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    decision craft_queue.runner_controls%ROWTYPE;
    current_revision bigint;
    now_at timestamptz;
    fenced bigint := 0;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    IF expected_revision IS NULL OR expected_revision NOT BETWEEN 0 AND 9223372036854775806
        OR COALESCE(desired_mode, '') NOT IN ('active', 'draining', 'stopped')
        OR length(btrim(COALESCE(evidence_ref, ''))) NOT BETWEEN 1 AND 500 THEN
        RETURN NULL;
    END IF;
    SELECT * INTO decision FROM craft_queue.runner_controls AS c
    WHERE c.runner_id = runner AND c.revision = expected_revision + 1;
    IF FOUND THEN
        IF decision.actor = actor_id AND decision.mode = desired_mode AND decision.evidence = evidence_ref THEN
            RETURN to_jsonb(decision);
        END IF;
        RETURN NULL;
    END IF;
    SELECT COALESCE(max(c.revision), 0) INTO current_revision FROM craft_queue.runner_controls AS c
    WHERE c.runner_id = runner;
    IF current_revision <> expected_revision OR NOT EXISTS (
        SELECT 1 FROM craft_queue.runner_repositories AS e WHERE e.runner_id = runner
    ) THEN RETURN NULL; END IF;

    now_at := clock_timestamp();
    IF desired_mode = 'stopped' THEN
        WITH stopped AS (
            UPDATE craft_queue.attempts AS a SET expires_at = now_at
            FROM craft_queue.work AS w
            WHERE a.runner_id = runner AND a.work_id = w.id AND a.generation = w.generation
                AND w.state = 'leased' AND a.outcome = 'leased' AND a.expires_at > now_at
            RETURNING a.work_id
        ), withdrawn AS (
            UPDATE craft_queue.profile_fallbacks AS f SET revoked_at = now_at, revoked_by = 'runner-stop:' || actor_id
            FROM stopped WHERE f.source_work_id = stopped.work_id AND f.applied_at IS NULL AND f.revoked_at IS NULL
        )
        SELECT count(*) INTO fenced FROM stopped;
    END IF;

    INSERT INTO craft_queue.runner_controls (runner_id, revision, mode, actor, evidence, recorded_at, fenced_leases)
    VALUES (runner, current_revision + 1, desired_mode, actor_id, evidence_ref, now_at, fenced)
    RETURNING * INTO decision;
    RETURN to_jsonb(decision);
END;
$$;

CREATE OR REPLACE FUNCTION craft_queue.claim(
    runner text, request uuid, context_capable boolean, profile_capable boolean, available_repositories text[]
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

REVOKE ALL ON FUNCTION craft_queue.runner_status(text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.set_runner_mode(text, text, bigint, text, text) FROM PUBLIC;
COMMIT;
