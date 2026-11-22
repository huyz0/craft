BEGIN;

CREATE OR REPLACE FUNCTION craft_queue.approve_dependency_evidence(
    actor text, approval_id text, order_id text, candidate_generation bigint, candidate_head text,
    previous_id text, approval_ref text, evidence_pins jsonb
) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    existing craft_queue.dependency_evidence_approvals%ROWTYPE;
    candidate craft_queue.work%ROWTYPE;
    latest craft_queue.verifications%ROWTYPE;
    active_id text;
    active_pins jsonb;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    IF COALESCE(approval_id, '') !~ '^[A-Za-z0-9_-]{1,100}$'
        OR length(btrim(COALESCE(approval_ref, ''))) NOT BETWEEN 1 AND 500
        OR EXISTS (SELECT 1 FROM unnest(ARRAY['intake_digest','policy_digest',
            'baseline_run_digest','candidate_run_digest']) AS fields(name)
            WHERE COALESCE(evidence_pins->>fields.name, '') !~ '^sha256:[a-f0-9]{64}$')
        OR evidence_pins->>'baseline_run_digest' = evidence_pins->>'candidate_run_digest'
        OR NOT craft_queue.dependency_scan_fresh(evidence_pins, clock_timestamp())
        OR (evidence_pins ? 'scanner_refresh_digest' AND (
            COALESCE(evidence_pins->>'scanner_refresh_digest', '') !~ '^sha256:[a-f0-9]{64}$'
            OR NOT (evidence_pins ? 'scanner_fresh_through')
        ))
        THEN RETURN NULL; END IF;
    SELECT * INTO existing FROM craft_queue.dependency_evidence_approvals AS a WHERE a.id = approval_id;
    IF FOUND THEN
        IF existing.work_id = order_id AND existing.generation = candidate_generation
            AND existing.head = candidate_head AND existing.operator_id = actor
            AND existing.previous_approval_id IS NOT DISTINCT FROM previous_id
            AND existing.approval_reference = approval_ref AND existing.pins = evidence_pins
            THEN RETURN to_jsonb(existing); END IF;
        RETURN NULL;
    END IF;

    SELECT w.* INTO candidate FROM craft_queue.work AS w
    JOIN craft_queue.attempts AS a ON a.work_id = w.id AND a.generation = w.generation
    JOIN craft_queue.candidate_packs AS p ON p.work_id = w.id AND p.generation = w.generation
        AND p.digest = w.candidate_evidence
    WHERE w.id = order_id AND w.state = 'candidate' AND w.generation = candidate_generation
        AND w.candidate = candidate_head AND a.runner_id <> actor
        AND w.payload->>'dependency_intake_digest' = evidence_pins->>'intake_digest';
    IF NOT FOUND OR EXISTS (SELECT 1 FROM craft_queue.recoveries AS r WHERE r.finished_at IS NULL)
        THEN RETURN NULL; END IF;

    SELECT id, pins INTO active_id, active_pins FROM craft_queue.dependency_evidence_approvals AS a
    WHERE a.work_id = order_id AND a.generation = candidate_generation ORDER BY a.sequence DESC LIMIT 1;
    IF active_id IS DISTINCT FROM previous_id THEN RETURN NULL; END IF;
    IF active_pins ? 'scanner_refresh_digest' AND NOT (evidence_pins ? 'scanner_refresh_digest')
        THEN RETURN NULL; END IF;
    IF evidence_pins->>'scanner_refresh_digest' IS DISTINCT FROM active_pins->>'scanner_refresh_digest'
        AND (evidence_pins->>'scanner_fresh_through')::bigint <= (active_pins->>'scanner_fresh_through')::bigint
        THEN RETURN NULL; END IF;

    SELECT * INTO latest FROM craft_queue.verifications AS v WHERE v.work_id = order_id
        AND v.generation = candidate_generation ORDER BY v.sequence DESC LIMIT 1;
    IF latest.passed THEN
        INSERT INTO craft_queue.verifications (work_id, generation, head, digest, verifier, passed, evidence, note)
        VALUES (order_id, candidate_generation, candidate_head, candidate.candidate_evidence, actor, false,
            'dependency-compatibility:approval:' || approval_id, 'Dependency evidence approval awaits independent verification');
    END IF;

    INSERT INTO craft_queue.dependency_evidence_approvals (id, work_id, generation, head, pack_digest,
        previous_approval_id, operator_id, approval_reference, pins)
    VALUES (approval_id, order_id, candidate_generation, candidate_head, candidate.candidate_evidence,
        previous_id, actor, approval_ref, evidence_pins)
    RETURNING * INTO existing;
    RETURN to_jsonb(existing);
END;
$$;

COMMIT;
