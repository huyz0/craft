BEGIN;

ALTER TABLE craft_queue.work ADD CONSTRAINT valid_dependency_intake CHECK (
    NOT (payload ? 'dependency_intake_digest') OR (
        COALESCE(payload->>'dependency_intake_digest', '') ~ '^sha256:[a-f0-9]{64}$'
        AND payload->'require_native_checks' IS NOT DISTINCT FROM 'true'::jsonb
        AND payload->>'ci_failure' IS NULL
    )
);

CREATE TABLE craft_queue.dependency_verifications (
    verification_sequence bigint PRIMARY KEY REFERENCES craft_queue.verifications(sequence),
    proof jsonb NOT NULL
);

ALTER FUNCTION craft_queue.record_verification(text, text, bigint, text, text, bigint, boolean, text, text)
    RENAME TO record_verification_receipt;

CREATE FUNCTION craft_queue.record_verification(
    actor text, order_id text, candidate_generation bigint, candidate_head text, pack_digest text,
    previous_sequence bigint, decision_passed boolean, evidence_ref text, decision_note text
) RETURNS bigint
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    IF evidence_ref LIKE 'dependency-compatibility:%' OR (decision_passed AND EXISTS (
        SELECT 1 FROM craft_queue.work AS w WHERE w.id = order_id AND w.payload ? 'dependency_intake_digest'
    )) THEN RETURN NULL; END IF;
    RETURN craft_queue.record_verification_receipt(actor, order_id, candidate_generation, candidate_head,
        pack_digest, previous_sequence, decision_passed, evidence_ref, decision_note);
END;
$$;

CREATE FUNCTION craft_queue.record_dependency_verification(
    actor text, order_id text, candidate_generation bigint, candidate_head text, pack_digest text,
    previous_sequence bigint, decision_passed boolean, proof_data jsonb
) RETURNS bigint
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    recorded bigint;
    previous jsonb;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    IF NOT EXISTS (SELECT 1 FROM craft_queue.work AS w
        WHERE w.id = order_id AND w.payload->>'dependency_intake_digest' = proof_data->>'intake_digest')
        OR EXISTS (SELECT 1 FROM unnest(ARRAY['intake_digest','policy_digest','baseline_run_digest',
            'candidate_run_digest','report_digest']) AS fields(name)
            WHERE COALESCE(proof_data->>fields.name, '') !~ '^sha256:[a-f0-9]{64}$')
        OR proof_data->>'baseline_run_digest' = proof_data->>'candidate_run_digest'
        THEN RETURN NULL; END IF;
    recorded := craft_queue.record_verification_receipt(actor, order_id, candidate_generation, candidate_head,
        pack_digest, previous_sequence, decision_passed, 'dependency-compatibility:' || (proof_data->>'report_digest'),
        'Approved dependency compatibility evidence');
    IF recorded IS NULL THEN RETURN NULL; END IF;
    SELECT proof INTO previous FROM craft_queue.dependency_verifications AS v
    WHERE v.verification_sequence = recorded;
    IF FOUND THEN
        IF previous IS DISTINCT FROM proof_data THEN RETURN NULL; END IF;
    ELSE
        INSERT INTO craft_queue.dependency_verifications (verification_sequence, proof)
        VALUES (recorded, proof_data);
    END IF;
    RETURN recorded;
END;
$$;

CREATE FUNCTION craft_queue.dependency_verifications_for(order_id text) RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT COALESCE(jsonb_agg(to_jsonb(verified) ORDER BY verification_sequence DESC), '[]'::jsonb)
    FROM (
        SELECT evidence.*, v.head, v.generation, v.passed
        FROM craft_queue.dependency_verifications AS evidence
        JOIN craft_queue.verifications AS v ON v.sequence = evidence.verification_sequence
        WHERE v.work_id = order_id ORDER BY v.sequence DESC LIMIT 20
    ) AS verified;
$$;

REVOKE ALL ON FUNCTION craft_queue.record_verification(text, text, bigint, text, text, bigint, boolean, text, text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.record_dependency_verification(text, text, bigint, text, text, bigint, boolean, jsonb) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.dependency_verifications_for(text) FROM PUBLIC;
COMMIT;
