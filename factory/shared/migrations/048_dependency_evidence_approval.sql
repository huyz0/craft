BEGIN;

CREATE TABLE craft_queue.dependency_evidence_approvals (
    sequence bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id text NOT NULL UNIQUE,
    work_id text NOT NULL,
    generation bigint NOT NULL,
    head text NOT NULL,
    pack_digest text NOT NULL,
    previous_approval_id text REFERENCES craft_queue.dependency_evidence_approvals(id),
    operator_id text NOT NULL,
    approval_reference text NOT NULL,
    pins jsonb NOT NULL,
    approved_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    FOREIGN KEY (work_id, generation) REFERENCES craft_queue.candidate_packs(work_id, generation)
);
CREATE INDEX dependency_approval_by_candidate
    ON craft_queue.dependency_evidence_approvals (work_id, generation, sequence DESC);

ALTER TABLE craft_queue.dependency_verifications ADD COLUMN approval_id text
    REFERENCES craft_queue.dependency_evidence_approvals(id);

CREATE FUNCTION craft_queue.approve_dependency_evidence(
    actor text, approval_id text, order_id text, candidate_generation bigint, candidate_head text,
    previous_id text, approval_ref text, evidence_pins jsonb
) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    existing craft_queue.dependency_evidence_approvals%ROWTYPE;
    candidate craft_queue.work%ROWTYPE;
    latest craft_queue.verifications%ROWTYPE;
    active_id text;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    IF COALESCE(approval_id, '') !~ '^[A-Za-z0-9_-]{1,100}$'
        OR length(btrim(COALESCE(approval_ref, ''))) NOT BETWEEN 1 AND 500
        OR EXISTS (SELECT 1 FROM unnest(ARRAY['intake_digest','policy_digest',
            'baseline_run_digest','candidate_run_digest']) AS fields(name)
            WHERE COALESCE(evidence_pins->>fields.name, '') !~ '^sha256:[a-f0-9]{64}$')
        OR evidence_pins->>'baseline_run_digest' = evidence_pins->>'candidate_run_digest'
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

    SELECT id INTO active_id FROM craft_queue.dependency_evidence_approvals AS a
    WHERE a.work_id = order_id AND a.generation = candidate_generation ORDER BY a.sequence DESC LIMIT 1;
    IF active_id IS DISTINCT FROM previous_id THEN RETURN NULL; END IF;

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

DROP FUNCTION craft_queue.record_dependency_verification(text, text, bigint, text, text, bigint, boolean, jsonb);

CREATE FUNCTION craft_queue.record_dependency_verification(
    actor text, order_id text, candidate_generation bigint, candidate_head text, pack_digest text,
    previous_sequence bigint, decision_passed boolean, approval_id text, proof_data jsonb
) RETURNS bigint
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    active craft_queue.dependency_evidence_approvals%ROWTYPE;
    recorded bigint;
    prior craft_queue.dependency_verifications%ROWTYPE;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    SELECT * INTO active FROM craft_queue.dependency_evidence_approvals AS a
    WHERE a.work_id = order_id AND a.generation = candidate_generation ORDER BY a.sequence DESC LIMIT 1;
    IF NOT FOUND OR active.id IS DISTINCT FROM approval_id OR active.operator_id = actor
        OR active.head IS DISTINCT FROM candidate_head OR active.pack_digest IS DISTINCT FROM pack_digest
        OR active.pins IS DISTINCT FROM (proof_data - 'report_digest')
        OR COALESCE(proof_data->>'report_digest', '') !~ '^sha256:[a-f0-9]{64}$'
        OR NOT EXISTS (SELECT 1 FROM craft_queue.work AS w WHERE w.id = order_id
            AND w.payload->>'dependency_intake_digest' = active.pins->>'intake_digest')
        OR EXISTS (SELECT 1 FROM craft_queue.recoveries AS r WHERE r.finished_at IS NULL)
        THEN RETURN NULL; END IF;
    recorded := craft_queue.record_verification_receipt(actor, order_id, candidate_generation, candidate_head,
        pack_digest, previous_sequence, decision_passed,
        'dependency-compatibility:' || approval_id || ':' || (proof_data->>'report_digest'),
        'Approved dependency compatibility evidence');
    IF recorded IS NULL THEN RETURN NULL; END IF;
    SELECT * INTO prior FROM craft_queue.dependency_verifications AS v WHERE v.verification_sequence = recorded;
    IF FOUND THEN
        IF prior.proof IS DISTINCT FROM proof_data OR prior.approval_id IS DISTINCT FROM approval_id THEN RETURN NULL; END IF;
    ELSE
        INSERT INTO craft_queue.dependency_verifications (verification_sequence, proof, approval_id)
        VALUES (recorded, proof_data, approval_id);
    END IF;
    RETURN recorded;
END;
$$;

CREATE FUNCTION craft_queue.dependency_approvals_for(order_id text) RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT COALESCE(jsonb_agg(to_jsonb(approved) ORDER BY sequence DESC), '[]'::jsonb)
    FROM (SELECT * FROM craft_queue.dependency_evidence_approvals AS a WHERE a.work_id = order_id
          ORDER BY a.sequence DESC LIMIT 20) AS approved;
$$;

REVOKE ALL ON FUNCTION craft_queue.approve_dependency_evidence(text, text, text, bigint, text, text, text, jsonb) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.record_dependency_verification(text, text, bigint, text, text, bigint, boolean, text, jsonb) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.dependency_approvals_for(text) FROM PUBLIC;

INSERT INTO craft_queue.verifications (work_id, generation, head, digest, verifier, passed, evidence, note)
SELECT w.id, w.generation, w.candidate, w.candidate_evidence, 'dependency-evidence-migration', false,
    'dependency-compatibility:migration:' || latest.sequence::text,
    'Dependency evidence now requires a separate operator approval'
FROM craft_queue.work AS w
JOIN LATERAL (
    SELECT v.sequence, v.passed FROM craft_queue.verifications AS v
    WHERE v.work_id = w.id AND v.generation = w.generation ORDER BY v.sequence DESC LIMIT 1
) AS latest ON latest.passed
WHERE w.state = 'candidate' AND w.payload ? 'dependency_intake_digest';

COMMIT;
