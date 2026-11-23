BEGIN;

CREATE TABLE craft_queue.dispatch_priorities (
    work_id text NOT NULL REFERENCES craft_queue.work(id),
    revision bigint NOT NULL CHECK (revision > 0),
    priority text NOT NULL CHECK (priority IN ('normal', 'expedited')),
    actor text NOT NULL,
    evidence text NOT NULL,
    recorded_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (work_id, revision)
);

CREATE FUNCTION craft_queue.set_dispatch_priority(
    actor_id text, order_id text, expected_revision bigint, desired_priority text, evidence_ref text
) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    decision craft_queue.dispatch_priorities%ROWTYPE;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    IF expected_revision IS NULL OR expected_revision NOT BETWEEN 0 AND 9223372036854775806
        OR COALESCE(desired_priority, '') NOT IN ('normal', 'expedited')
        OR length(btrim(COALESCE(evidence_ref, ''))) NOT BETWEEN 1 AND 500 THEN
        RETURN NULL;
    END IF;
    SELECT d.* INTO decision FROM craft_queue.dispatch_priorities AS d
    WHERE d.work_id = order_id AND d.revision = expected_revision + 1;
    IF FOUND THEN
        IF decision.actor = actor_id AND decision.priority = desired_priority AND decision.evidence = evidence_ref
            THEN RETURN to_jsonb(decision); END IF;
        RETURN NULL;
    END IF;
    IF expected_revision <> COALESCE((
        SELECT max(d.revision) FROM craft_queue.dispatch_priorities AS d WHERE d.work_id = order_id
    ), 0) OR NOT EXISTS (
        SELECT 1 FROM craft_queue.work AS w
        LEFT JOIN craft_queue.attempts AS a ON a.work_id = w.id AND a.generation = w.generation
        WHERE w.id = order_id AND (w.state = 'queued'
            OR (w.state = 'leased' AND a.outcome = 'leased' AND a.expires_at <= clock_timestamp()))
    ) THEN RETURN NULL; END IF;

    INSERT INTO craft_queue.dispatch_priorities (work_id, revision, priority, actor, evidence)
    VALUES (order_id, expected_revision + 1, desired_priority, actor_id, evidence_ref)
    RETURNING * INTO decision;
    RETURN to_jsonb(decision);
END;
$$;

REVOKE ALL ON FUNCTION craft_queue.set_dispatch_priority(text, text, bigint, text, text) FROM PUBLIC;

ALTER FUNCTION craft_queue.dispatch_status(text) RENAME TO dispatch_status_before_priority;

CREATE FUNCTION craft_queue.dispatch_status(order_id text) RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT craft_queue.dispatch_status_before_priority(order_id) || jsonb_build_object(
        'dispatch_priority', COALESCE(history.decisions->0->>'priority', 'normal'),
        'dispatch_priority_revision', COALESCE((history.decisions->0->>'revision')::bigint, 0),
        'dispatch_priority_history', history.decisions
    )
    FROM (
        SELECT COALESCE(jsonb_agg(to_jsonb(d) ORDER BY d.revision DESC), '[]'::jsonb) AS decisions
        FROM (SELECT * FROM craft_queue.dispatch_priorities WHERE work_id = order_id
              ORDER BY revision DESC LIMIT 10) AS d
    ) AS history;
$$;

REVOKE ALL ON FUNCTION craft_queue.dispatch_status(text) FROM PUBLIC;

CREATE OR REPLACE FUNCTION craft_queue.claim_before_poll_receipts(
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
    approval_id text;
    approval_deadline timestamptz;
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
          AND (selected_adapter <> 'claude' OR previous.claude_approval_id IS NOT NULL)
          AND (available_repositories IS NULL OR w.repository = ANY(available_repositories))
          AND craft_queue.standing_runner_allowed(w.id, runner, now_at)
          AND craft_queue.series_blocker(w.id) IS NULL
          AND (profile_capable OR w.payload->>'execution_profile' IS NULL)
          AND (context_capable OR NOT EXISTS (SELECT 1 FROM craft_queue.project_context_pins(w.payload)));
        RETURN;
    END IF;
    IF settings.paused OR mode = 'draining' THEN RETURN; END IF;

    -- Return review capacity held by attempts whose repository approval expired.
    WITH expired AS (
        UPDATE craft_queue.attempts AS a SET outcome = 'expired'
        FROM craft_queue.claude_repository_approvals AS g
        WHERE a.claude_approval_id = g.grant_id AND a.outcome = 'leased' AND g.expires_at <= now_at
        RETURNING a.work_id, a.generation
    )
    UPDATE craft_queue.work AS w SET state = 'queued'
    FROM expired AS a WHERE w.id = a.work_id AND w.generation = a.generation AND w.state = 'leased';

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
    LEFT JOIN LATERAL (
        SELECT d.priority FROM craft_queue.dispatch_priorities AS d
        WHERE d.work_id = w.id ORDER BY d.revision DESC LIMIT 1
    ) AS policy ON true
    WHERE grant_row.runner_id = runner AND w.reserved_tokens <= tokens_free
      AND COALESCE(w.payload->>'agent_adapter', 'codex') = selected_adapter
      AND (selected_adapter <> 'claude' OR EXISTS (
          SELECT 1 FROM craft_queue.claude_repository_approvals AS g
          WHERE g.repository = w.repository AND g.revoked_at IS NULL AND g.expires_at > now_at))
      AND (available_repositories IS NULL OR w.repository = ANY(available_repositories))
      AND craft_queue.standing_runner_allowed(w.id, runner, now_at)
      AND craft_queue.series_blocker(w.id) IS NULL
      AND (w.payload->>'execution_profile' IS NULL OR (profile_capable
        AND w.generation < (w.payload->'execution_profile'->>'max_attempts')::bigint))
      AND (context_capable OR NOT EXISTS (SELECT 1 FROM craft_queue.project_context_pins(w.payload)))
      AND ((w.state = 'queued' AND (review_free OR w.repair_from_generation IS NOT NULL OR standing.work_id IS NOT NULL))
        OR (w.state = 'leased' AND a.outcome = 'leased' AND a.expires_at <= now_at))
    ORDER BY (w.state = 'leased' OR w.repair_from_generation IS NOT NULL OR standing.work_id IS NOT NULL) DESC,
        COALESCE(policy.priority = 'expedited', false) DESC, w.created_at, w.id
    LIMIT 1;
    IF NOT FOUND THEN RETURN; END IF;

    IF selected_adapter = 'claude' THEN
        SELECT g.grant_id, g.expires_at INTO STRICT approval_id, approval_deadline
        FROM craft_queue.claude_repository_approvals AS g
        WHERE g.repository = selected.repository AND g.revoked_at IS NULL AND g.expires_at > now_at;
    END IF;

    UPDATE craft_queue.attempts AS a SET outcome = 'expired'
    WHERE a.work_id = selected.id AND a.generation = selected.generation AND a.outcome = 'leased';
    UPDATE craft_queue.work AS w SET state = 'leased', generation = w.generation + 1
    WHERE w.id = selected.id RETURNING w.* INTO selected;
    deadline := LEAST(now_at + make_interval(secs => settings.lease_seconds), approval_deadline);
    INSERT INTO craft_queue.attempts (work_id, generation, runner_id, request_id, expires_at, started_at,
        token_limit, claude_approval_id)
    VALUES (selected.id, selected.generation, runner, request, deadline, now_at, selected.reserved_tokens, approval_id);
    RETURN QUERY SELECT selected.id, selected.repository, selected.generation, deadline,
        (selected.payload - 'repository') || CASE WHEN selected.repair_from_generation IS NULL
            THEN '{}'::jsonb ELSE '{"shared_repair":true}'::jsonb END;
END;
$$;

COMMIT;
