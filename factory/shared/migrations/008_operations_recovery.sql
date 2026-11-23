BEGIN;

CREATE TABLE craft_queue.recoveries (
    epoch uuid PRIMARY KEY,
    started_by text NOT NULL,
    started_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    finished_by text,
    finished_at timestamptz,
    evidence text
);
CREATE UNIQUE INDEX recoveries_one_active ON craft_queue.recoveries ((finished_at IS NULL))
    WHERE finished_at IS NULL;

CREATE FUNCTION craft_queue.begin_recovery(recovery_id uuid, actor text) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    recovered craft_queue.recoveries%ROWTYPE;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    SELECT * INTO recovered FROM craft_queue.recoveries AS r WHERE r.epoch = recovery_id;
    IF FOUND THEN RETURN to_jsonb(recovered); END IF;
    IF EXISTS (SELECT 1 FROM craft_queue.recoveries AS r WHERE r.finished_at IS NULL) THEN
        RAISE EXCEPTION 'another recovery is already active';
    END IF;
    INSERT INTO craft_queue.recoveries (epoch, started_by)
    VALUES (recovery_id, actor) RETURNING * INTO recovered;
    UPDATE craft_queue.control SET paused = true WHERE singleton;
    UPDATE craft_queue.draft_policies SET enabled = false WHERE enabled;

    UPDATE craft_queue.attempts AS a
    SET outcome = 'expired', expires_at = LEAST(a.expires_at, recovered.started_at)
    FROM craft_queue.work AS w
    WHERE w.state = 'leased' AND a.work_id = w.id AND a.generation = w.generation AND a.outcome = 'leased';
    UPDATE craft_queue.work SET state = 'queued' WHERE state = 'leased';

    WITH held AS (
        UPDATE craft_queue.publications SET state = 'uncertain'
        WHERE state IN ('reserved', 'sending') RETURNING action_key
    )
    INSERT INTO craft_queue.publication_events (action_key, state, actor, evidence)
    SELECT action_key, 'uncertain', actor, 'Restore recovery ' || recovery_id::text FROM held;
    RETURN to_jsonb(recovered);
END;
$$;

CREATE FUNCTION craft_queue.finish_recovery(recovery_id uuid, actor text, evidence_ref text) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    recovered craft_queue.recoveries%ROWTYPE;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    SELECT * INTO recovered FROM craft_queue.recoveries AS r WHERE r.epoch = recovery_id;
    IF NOT FOUND THEN RETURN NULL; END IF;
    IF length(btrim(COALESCE(evidence_ref, ''))) NOT BETWEEN 1 AND 1000 THEN
        RAISE EXCEPTION 'a recovery evidence reference is required';
    END IF;
    IF recovered.finished_at IS NOT NULL THEN
        IF recovered.finished_by = actor AND recovered.evidence = evidence_ref THEN
            RETURN to_jsonb(recovered);
        END IF;
        RETURN NULL;
    END IF;
    IF EXISTS (SELECT 1 FROM craft_queue.publications AS p WHERE p.state IN ('reserved','sending','uncertain')) THEN
        RETURN NULL;
    END IF;
    UPDATE craft_queue.recoveries AS r
    SET finished_at = clock_timestamp(), finished_by = actor, evidence = evidence_ref
    WHERE r.epoch = recovery_id RETURNING * INTO recovered;
    RETURN to_jsonb(recovered);
END;
$$;

CREATE FUNCTION craft_queue.guard_recovery()
RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog AS $$
BEGIN
    IF EXISTS (SELECT 1 FROM craft_queue.recoveries AS r WHERE r.finished_at IS NULL) THEN
        IF TG_TABLE_NAME = 'cutovers' THEN
            RAISE EXCEPTION 'finish restore recovery before changing authority';
        END IF;
        IF NOT NEW.paused THEN
            RAISE EXCEPTION 'finish restore recovery before resuming admission';
        END IF;
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER guard_recovery_resume BEFORE UPDATE ON craft_queue.control
    FOR EACH ROW EXECUTE FUNCTION craft_queue.guard_recovery();
CREATE TRIGGER guard_recovery_cutover BEFORE INSERT OR UPDATE ON craft_queue.cutovers
    FOR EACH ROW EXECUTE FUNCTION craft_queue.guard_recovery();

CREATE FUNCTION craft_queue.operation_summary() RETURNS jsonb
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    WITH sample AS (SELECT statement_timestamp() AS at)
    SELECT jsonb_build_object(
        'paused', c.paused,
        'recovery_active', EXISTS(SELECT 1 FROM craft_queue.recoveries AS r WHERE r.finished_at IS NULL),
        'review_limit', c.review_limit,
        'review_reserved', (SELECT count(*) FROM craft_queue.work AS w WHERE w.state IN ('leased','candidate')),
        'queued', (SELECT count(*) FROM craft_queue.work AS w WHERE w.state = 'queued'),
        'oldest_queued_seconds', (SELECT floor(extract(epoch FROM sampled.at - min(w.created_at)))
            FROM craft_queue.work AS w WHERE w.state = 'queued'),
        'live_leases', (SELECT count(*) FROM craft_queue.attempts AS a
            WHERE a.outcome = 'leased' AND a.expires_at > sampled.at),
        'overdue_leases', (SELECT count(*) FROM craft_queue.attempts AS a
            WHERE a.outcome = 'leased' AND a.expires_at <= sampled.at),
        'oldest_live_lease_seconds', (SELECT floor(extract(epoch FROM sampled.at - min(a.started_at)))
            FROM craft_queue.attempts AS a WHERE a.outcome = 'leased' AND a.expires_at > sampled.at),
        'tokens_charged_or_reserved', (SELECT COALESCE(sum(COALESCE(a.reported_tokens, a.token_limit)), 0)
            FROM craft_queue.attempts AS a WHERE a.started_at > sampled.at - interval '24 hours'
                OR (a.outcome = 'leased' AND a.expires_at > sampled.at)),
        'token_budget', c.token_budget,
        'held_publications', (SELECT count(*) FROM craft_queue.publications AS p
            WHERE p.state IN ('sending','uncertain'))
    )
    FROM craft_queue.control AS c CROSS JOIN sample AS sampled;
$$;

CREATE FUNCTION craft_queue.work_audit(order_id text) RETURNS jsonb
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT jsonb_build_object(
        'work', craft_queue.status(w.id),
        'attempts', COALESCE((
            SELECT jsonb_agg(to_jsonb(a) ORDER BY a.generation DESC) FROM (
                SELECT generation, runner_id, started_at, expires_at, token_limit, reported_tokens, outcome
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

CREATE FUNCTION craft_queue.publication_holds() RETURNS jsonb
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT COALESCE(jsonb_agg(to_jsonb(h) ORDER BY h.first_held_at), '[]'::jsonb)
    FROM (
        SELECT p.work_id, p.action_key, p.repository, p.state,
               (SELECT min(e.observed_at) FROM craft_queue.publication_events AS e
                WHERE e.action_key = p.action_key AND e.state IN ('sending','uncertain')) AS first_held_at
        FROM craft_queue.publications AS p WHERE p.state IN ('sending','uncertain')
        ORDER BY first_held_at LIMIT 64
    ) AS h;
$$;

REVOKE ALL ON FUNCTION craft_queue.begin_recovery(uuid, text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.finish_recovery(uuid, text, text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.guard_recovery() FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.operation_summary() FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.work_audit(text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.publication_holds() FROM PUBLIC;

COMMIT;
