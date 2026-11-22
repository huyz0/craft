BEGIN;

CREATE TABLE craft_queue.claude_repository_approvals (
    grant_id text PRIMARY KEY,
    repository text NOT NULL,
    approved_by text NOT NULL,
    approval_reference text NOT NULL,
    approved_at timestamptz NOT NULL,
    expires_at timestamptz NOT NULL CHECK (expires_at > approved_at),
    revoked_at timestamptz,
    revoked_by text,
    revocation_reference text
);
CREATE INDEX claude_repository_approvals_active ON craft_queue.claude_repository_approvals (repository, expires_at)
WHERE revoked_at IS NULL;

ALTER TABLE craft_queue.attempts ADD COLUMN claude_approval_id text
    REFERENCES craft_queue.claude_repository_approvals (grant_id);
CREATE INDEX attempts_claude_approval ON craft_queue.attempts (claude_approval_id)
WHERE outcome = 'leased' AND claude_approval_id IS NOT NULL;

-- Attempts issued before the repository approval existed must obtain a new lease.
WITH expired AS (
    UPDATE craft_queue.attempts AS a
    SET outcome = 'expired', expires_at = LEAST(a.expires_at, clock_timestamp())
    FROM craft_queue.work AS w
    WHERE w.id = a.work_id AND w.generation = a.generation AND w.state = 'leased'
      AND a.outcome = 'leased' AND w.payload->>'agent_adapter' = 'claude'
    RETURNING a.work_id, a.generation
)
UPDATE craft_queue.work AS w SET state = 'queued'
FROM expired AS a WHERE w.id = a.work_id AND w.generation = a.generation;

CREATE FUNCTION craft_queue.approve_claude_repository(actor text, repo text, id text, evidence text, lifetime_seconds integer)
RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    approval craft_queue.claude_repository_approvals%ROWTYPE;
    now_at timestamptz;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    IF COALESCE(id, '') !~ '^[A-Za-z0-9_-]{1,100}$'
        OR length(btrim(COALESCE(repo, ''))) NOT BETWEEN 1 AND 200
        OR length(btrim(COALESCE(evidence, ''))) NOT BETWEEN 1 AND 500
        OR lifetime_seconds IS NULL OR lifetime_seconds NOT BETWEEN 1 AND 2592000 THEN RETURN NULL; END IF;
    SELECT * INTO approval FROM craft_queue.claude_repository_approvals AS g WHERE g.grant_id = id;
    IF FOUND THEN
        IF approval.repository = repo AND approval.approved_by = actor AND approval.approval_reference = evidence
            AND approval.expires_at - approval.approved_at = make_interval(secs => lifetime_seconds) THEN
            RETURN to_jsonb(approval);
        END IF;
        RETURN NULL;
    END IF;

    now_at := clock_timestamp();
    IF EXISTS (SELECT 1 FROM craft_queue.recoveries AS r WHERE r.finished_at IS NULL)
        OR EXISTS (SELECT 1 FROM craft_queue.claude_repository_approvals AS g
                   WHERE g.repository = repo AND g.revoked_at IS NULL AND g.expires_at > now_at) THEN RETURN NULL; END IF;

    INSERT INTO craft_queue.claude_repository_approvals (grant_id, repository, approved_by, approval_reference,
        approved_at, expires_at)
    VALUES (id, repo, actor, evidence, now_at, now_at + make_interval(secs => lifetime_seconds))
    RETURNING * INTO approval;
    RETURN to_jsonb(approval);
END;
$$;

CREATE FUNCTION craft_queue.revoke_claude_repository(actor text, id text, evidence text) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    approval craft_queue.claude_repository_approvals%ROWTYPE;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    IF length(btrim(COALESCE(evidence, ''))) NOT BETWEEN 1 AND 500 THEN RETURN NULL; END IF;
    SELECT * INTO approval FROM craft_queue.claude_repository_approvals AS g WHERE g.grant_id = id;
    IF NOT FOUND THEN RETURN NULL; END IF;
    IF approval.revoked_at IS NOT NULL THEN
        IF approval.revoked_by = actor AND approval.revocation_reference = evidence THEN
            RETURN to_jsonb(approval);
        END IF;
        RETURN NULL;
    END IF;

    UPDATE craft_queue.claude_repository_approvals AS g
    SET revoked_at = clock_timestamp(), revoked_by = actor, revocation_reference = evidence
    WHERE g.grant_id = id RETURNING * INTO approval;
    WITH expired AS (
        UPDATE craft_queue.attempts AS a
        SET outcome = 'expired', expires_at = LEAST(a.expires_at, approval.revoked_at)
        WHERE a.claude_approval_id = id AND a.outcome = 'leased'
        RETURNING a.work_id, a.generation
    )
    UPDATE craft_queue.work AS w SET state = 'queued'
    FROM expired AS a WHERE w.id = a.work_id AND w.generation = a.generation AND w.state = 'leased';
    RETURN to_jsonb(approval);
END;
$$;

CREATE FUNCTION craft_queue.revoke_claude_repository_on_restore() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
BEGIN
    UPDATE craft_queue.claude_repository_approvals
    SET revoked_at = NEW.started_at, revoked_by = NEW.started_by, revocation_reference = 'restore:' || NEW.epoch::text
    WHERE revoked_at IS NULL;
    RETURN NEW;
END;
$$;
CREATE TRIGGER revoke_claude_repository_on_restore AFTER INSERT ON craft_queue.recoveries
FOR EACH ROW EXECUTE FUNCTION craft_queue.revoke_claude_repository_on_restore();

CREATE OR REPLACE FUNCTION craft_queue.claim(
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
        w.created_at, w.id
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

CREATE OR REPLACE FUNCTION craft_queue.renew(runner text, lease_work_id text, lease_generation bigint)
RETURNS timestamptz
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    settings craft_queue.control%ROWTYPE;
    now_at timestamptz;
    renewed timestamptz;
BEGIN
    SELECT * INTO STRICT settings FROM craft_queue.control FOR UPDATE;
    now_at := clock_timestamp();
    UPDATE craft_queue.attempts AS a
    SET expires_at = LEAST(now_at + make_interval(secs => settings.lease_seconds),
        (SELECT g.expires_at FROM craft_queue.claude_repository_approvals AS g WHERE g.grant_id = a.claude_approval_id))
    FROM craft_queue.work AS w, craft_queue.runner_repositories AS grant_row
    WHERE w.id = lease_work_id AND w.state = 'leased' AND w.generation = lease_generation
      AND a.work_id = w.id AND a.generation = w.generation AND a.runner_id = runner
      AND a.outcome = 'leased' AND a.expires_at > now_at
      AND grant_row.runner_id = runner AND grant_row.repository = w.repository
      AND (w.payload->>'agent_adapter' IS DISTINCT FROM 'claude' OR a.claude_approval_id IS NOT NULL)
      AND craft_queue.series_blocker(w.id) IS NULL
    RETURNING a.expires_at INTO renewed;
    RETURN renewed;
END;
$$;

CREATE OR REPLACE FUNCTION craft_queue.work_audit(order_id text) RETURNS jsonb
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT jsonb_build_object(
        'work', craft_queue.status(w.id),
        'attempts', COALESCE((
            SELECT jsonb_agg(to_jsonb(a) ORDER BY a.generation DESC) FROM (
                SELECT generation, runner_id, started_at, expires_at, token_limit, reported_tokens, outcome,
                       claude_approval_id
                FROM craft_queue.attempts AS a WHERE a.work_id = w.id
                ORDER BY generation DESC LIMIT 64
            ) AS a
        ), '[]'::jsonb),
        'verifications', COALESCE((
            SELECT jsonb_agg(to_jsonb(v) ORDER BY v.sequence DESC) FROM (
                SELECT sequence, generation, head, digest, verifier, passed, evidence, observed_at
                FROM craft_queue.verifications AS v WHERE v.work_id = w.id
                ORDER BY sequence DESC LIMIT 64
            ) AS v
        ), '[]'::jsonb),
        'publications', COALESCE((
            SELECT jsonb_agg(to_jsonb(p) ORDER BY p.generation DESC) FROM (
                SELECT action_key, generation, head, state, result_url
                FROM craft_queue.publications AS p WHERE p.work_id = w.id
                ORDER BY generation DESC LIMIT 64
            ) AS p
        ), '[]'::jsonb)
    )
    FROM craft_queue.work AS w WHERE w.id = order_id;
$$;

REVOKE ALL ON FUNCTION craft_queue.approve_claude_repository(text, text, text, text, integer) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.revoke_claude_repository(text, text, text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.revoke_claude_repository_on_restore() FROM PUBLIC;

COMMIT;
