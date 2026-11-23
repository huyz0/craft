BEGIN;

CREATE TABLE craft_queue.github_work_closures (
    work_id text PRIMARY KEY REFERENCES craft_queue.work(id),
    merge_receipt_id text NOT NULL UNIQUE REFERENCES craft_queue.github_shadow_merges(receipt_id),
    head text NOT NULL,
    result_commit text NOT NULL,
    actor text NOT NULL,
    evidence text NOT NULL,
    closed_at timestamptz NOT NULL DEFAULT clock_timestamp()
);

CREATE FUNCTION craft_queue.close_github_work(
    actor_id text, order_id text, receipt text, expected_head text, evidence_ref text
) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    closure craft_queue.github_work_closures%ROWTYPE;
    result text;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    SELECT c.* INTO closure FROM craft_queue.github_work_closures AS c WHERE c.work_id = order_id;
    IF FOUND THEN
        IF closure.merge_receipt_id = receipt AND closure.head = expected_head
            AND closure.actor = actor_id AND closure.evidence = evidence_ref
            THEN RETURN to_jsonb(closure); END IF;
        RETURN NULL;
    END IF;
    IF length(btrim(COALESCE(evidence_ref, ''))) NOT BETWEEN 1 AND 500
        OR EXISTS (SELECT 1 FROM craft_queue.recoveries AS r WHERE r.finished_at IS NULL)
        OR EXISTS (SELECT 1 FROM craft_queue.series_members AS s WHERE s.work_id = order_id)
        OR EXISTS (SELECT 1 FROM craft_queue.publications AS p
                   WHERE p.work_id = order_id AND p.state IN ('reserved', 'sending', 'uncertain'))
        THEN RETURN NULL; END IF;
    SELECT m.observation->>'result_commit' INTO result
    FROM craft_queue.shadow_merge_receipts AS r
    JOIN craft_queue.github_shadow_merges AS m ON m.receipt_id = r.id
    WHERE r.id = receipt AND r.work_id = order_id AND r.head = expected_head
      AND m.observation->>'head' = expected_head
      AND r.publication_key = craft_queue.current_review_draft(r.work_id, r.generation, r.head, r.verification_sequence);
    IF NOT FOUND OR COALESCE(result, '') !~ '^([a-f0-9]{40}|[a-f0-9]{64})$' THEN RETURN NULL; END IF;

    INSERT INTO craft_queue.github_work_closures(work_id, merge_receipt_id, head, result_commit, actor, evidence)
    VALUES (order_id, receipt, expected_head, result, actor_id, evidence_ref)
    RETURNING * INTO closure;
    UPDATE craft_queue.work AS w SET state = 'released' WHERE w.id = order_id;
    RETURN to_jsonb(closure);
END;
$$;

REVOKE ALL ON FUNCTION craft_queue.close_github_work(text, text, text, text, text) FROM PUBLIC;

ALTER FUNCTION craft_queue.status(text) RENAME TO status_before_standalone_merge;

CREATE FUNCTION craft_queue.status(work_id text) RETURNS jsonb
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT craft_queue.status_before_standalone_merge($1) ||
        COALESCE((SELECT jsonb_build_object('state', 'merged', 'independent_checks_passed', true,
                                           'github_work_closure', to_jsonb(c))
                  FROM craft_queue.github_work_closures AS c WHERE c.work_id = $1), '{}'::jsonb);
$$;
REVOKE ALL ON FUNCTION craft_queue.status(text) FROM PUBLIC;

CREATE FUNCTION craft_queue.guard_github_work_closure() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog AS $$
BEGIN
    IF OLD.state = 'released' AND NEW.state <> 'released' AND EXISTS (
        SELECT 1 FROM craft_queue.github_work_closures AS c WHERE c.work_id = OLD.id
    ) THEN RAISE EXCEPTION 'standalone work has a confirmed GitHub merge'; END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER guard_github_work_closure BEFORE UPDATE OF state ON craft_queue.work
FOR EACH ROW EXECUTE FUNCTION craft_queue.guard_github_work_closure();

REVOKE ALL ON FUNCTION craft_queue.guard_github_work_closure() FROM PUBLIC;

CREATE OR REPLACE FUNCTION craft_queue.withdraw_series_on_restore() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog AS $$
BEGIN
    UPDATE craft_queue.series_observations
    SET withdrawn_at = clock_timestamp(), withdrawal_evidence = 'Restore recovery ' || NEW.epoch::text
    WHERE withdrawn_at IS NULL;
    UPDATE craft_queue.work AS w
    SET state = CASE WHEN craft_queue.team_skill_allowed(w.repository, w.payload)
        AND craft_queue.project_context_allowed(w.repository, w.payload) THEN 'candidate' ELSE 'cancelled' END,
        cancellation_reason = CASE WHEN craft_queue.team_skill_allowed(w.repository, w.payload)
        AND craft_queue.project_context_allowed(w.repository, w.payload) THEN NULL
        ELSE 'Release needs fresh observation after restore and required context or skill was retired' END
    WHERE w.state = 'released' AND NOT EXISTS (
        SELECT 1 FROM craft_queue.github_work_closures AS c WHERE c.work_id = w.id
    );
    RETURN NEW;
END;
$$;

COMMIT;
