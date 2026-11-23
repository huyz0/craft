BEGIN;

ALTER TABLE craft_queue.work DROP CONSTRAINT work_state_check;
ALTER TABLE craft_queue.work ADD CONSTRAINT work_state_check
    CHECK (state IN ('queued', 'leased', 'candidate', 'cancelled', 'released', 'blocked'));

CREATE TABLE craft_queue.ci_preflight_holds (
    work_id text NOT NULL,
    generation bigint NOT NULL,
    runner_id text NOT NULL,
    reason text NOT NULL CHECK (reason IN ('branch_moved', 'workflow_superseded')),
    recorded_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    released_at timestamptz,
    released_by text,
    release_evidence text,
    PRIMARY KEY(work_id, generation),
    FOREIGN KEY(work_id, generation) REFERENCES craft_queue.attempts(work_id, generation)
);

CREATE FUNCTION craft_queue.hold_obsolete_ci(runner text, order_id text, lease_generation bigint, reason_code text)
RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    hold craft_queue.ci_preflight_holds%ROWTYPE;
    now_at timestamptz;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    SELECT h.* INTO hold FROM craft_queue.ci_preflight_holds AS h
    JOIN craft_queue.work AS w ON w.id = h.work_id
    JOIN craft_queue.runner_repositories AS enrollment
      ON enrollment.repository = w.repository AND enrollment.runner_id = runner
    WHERE h.work_id = order_id AND h.generation = lease_generation;
    IF FOUND THEN
        IF hold.runner_id = runner AND hold.reason = reason_code THEN RETURN to_jsonb(hold); END IF;
        RETURN NULL;
    END IF;
    IF COALESCE(reason_code, '') NOT IN ('branch_moved', 'workflow_superseded') THEN RETURN NULL; END IF;
    now_at := clock_timestamp();
    UPDATE craft_queue.attempts AS a SET expires_at = now_at, outcome = 'expired', reported_tokens = 0
    FROM craft_queue.work AS w
    WHERE w.id = order_id AND a.work_id = w.id AND a.generation = lease_generation
      AND craft_queue.runner_context_allowed(w, a, runner, lease_generation)
      AND a.expires_at > now_at AND a.profile_started_at IS NULL AND a.reported_tokens IS NULL
      AND w.payload->>'ci_policy_digest' ~ '^sha256:[a-f0-9]{64}$'
      AND w.payload->'ci_failure' ? 'run_attempt'
      AND NOT EXISTS (SELECT 1 FROM craft_queue.recoveries AS r WHERE r.finished_at IS NULL);
    IF NOT FOUND THEN RETURN NULL; END IF;

    INSERT INTO craft_queue.ci_preflight_holds(work_id, generation, runner_id, reason)
    VALUES (order_id, lease_generation, runner, reason_code) RETURNING * INTO hold;
    UPDATE craft_queue.work AS w SET state = 'blocked' WHERE w.id = order_id;
    RETURN to_jsonb(hold);
END;
$$;

CREATE FUNCTION craft_queue.retry_ci_preflight(actor_id text, order_id text, held_generation bigint, evidence_ref text)
RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    hold craft_queue.ci_preflight_holds%ROWTYPE;
    work craft_queue.work%ROWTYPE;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    SELECT h.* INTO hold FROM craft_queue.ci_preflight_holds AS h
    WHERE h.work_id = order_id AND h.generation = held_generation;
    IF NOT FOUND THEN RETURN NULL; END IF;
    IF hold.released_at IS NOT NULL THEN
        IF hold.released_by = actor_id AND hold.release_evidence = evidence_ref THEN RETURN to_jsonb(hold); END IF;
        RETURN NULL;
    END IF;
    IF length(btrim(COALESCE(evidence_ref, ''))) NOT BETWEEN 1 AND 500
        OR EXISTS (SELECT 1 FROM craft_queue.recoveries AS r WHERE r.finished_at IS NULL) THEN RETURN NULL; END IF;
    SELECT w.* INTO work FROM craft_queue.work AS w
    WHERE w.id = order_id AND w.generation = held_generation AND w.state = 'blocked';
    IF NOT FOUND THEN RETURN NULL; END IF;
    IF (work.repair_from_generation IS NOT NULL OR
        EXISTS (SELECT 1 FROM craft_queue.standing_work AS s WHERE s.work_id = order_id))
        AND (craft_queue.operation_summary()->>'review_reserved')::bigint >=
            (SELECT review_limit FROM craft_queue.control)
        THEN RETURN NULL; END IF;
    UPDATE craft_queue.ci_preflight_holds AS h
    SET released_at = clock_timestamp(), released_by = actor_id, release_evidence = evidence_ref
    WHERE h.work_id = order_id AND h.generation = held_generation RETURNING * INTO hold;
    UPDATE craft_queue.work AS w SET state = 'queued' WHERE w.id = order_id;
    RETURN to_jsonb(hold);
END;
$$;

CREATE FUNCTION craft_queue.ci_preflight_holds_for(order_id text) RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT COALESCE(jsonb_agg(to_jsonb(h) ORDER BY h.generation DESC), '[]'::jsonb)
    FROM (SELECT * FROM craft_queue.ci_preflight_holds WHERE work_id = order_id
          ORDER BY generation DESC LIMIT 20) AS h;
$$;

ALTER FUNCTION craft_queue.status(text) RENAME TO status_before_ci_preflight;
CREATE FUNCTION craft_queue.status(work_id text) RETURNS jsonb
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT craft_queue.status_before_ci_preflight($1) ||
        COALESCE((SELECT jsonb_build_object('ci_preflight_hold', to_jsonb(h))
                  FROM craft_queue.ci_preflight_holds AS h JOIN craft_queue.work AS w ON w.id = h.work_id
                  WHERE w.id = $1 AND w.state = 'blocked' AND w.generation = h.generation), '{}'::jsonb);
$$;

ALTER FUNCTION craft_queue.dispatch_status(text) RENAME TO dispatch_status_before_ci_preflight;
CREATE FUNCTION craft_queue.dispatch_status(order_id text) RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT craft_queue.dispatch_status_before_ci_preflight($1) ||
        COALESCE((SELECT jsonb_build_object('waiting_for_claim', false,
             'blockers', jsonb_build_array('ci_preflight_hold'), 'ci_preflight_hold', to_jsonb(h))
             FROM craft_queue.ci_preflight_holds AS h JOIN craft_queue.work AS w ON w.id = h.work_id
             WHERE w.id = $1 AND w.state = 'blocked' AND w.generation = h.generation), '{}'::jsonb);
$$;

REVOKE ALL ON FUNCTION craft_queue.status(text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.dispatch_status(text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.hold_obsolete_ci(text,text,bigint,text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.retry_ci_preflight(text,text,bigint,text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.ci_preflight_holds_for(text) FROM PUBLIC;

COMMIT;
