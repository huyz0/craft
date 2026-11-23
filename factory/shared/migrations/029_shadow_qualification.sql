BEGIN;

CREATE TABLE craft_queue.shadow_outcomes (
    receipt_id text PRIMARY KEY REFERENCES craft_queue.shadow_merge_receipts(id),
    verifier text NOT NULL,
    basis text NOT NULL CHECK (basis IN ('fixture', 'forge_observed')),
    expected_eligible boolean NOT NULL,
    evidence_digest text NOT NULL CHECK (evidence_digest ~ '^sha256:[a-f0-9]{64}$'),
    evidence text NOT NULL,
    note text NOT NULL,
    recorded_at timestamptz NOT NULL DEFAULT clock_timestamp()
);

CREATE FUNCTION craft_queue.record_shadow_outcome(
    actor_id text, receipt text, candidate_head text, report_pin text,
    source text, expected boolean, evidence_pin text, evidence_ref text, review_note text
) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    evaluated craft_queue.shadow_merge_receipts%ROWTYPE;
    outcome craft_queue.shadow_outcomes%ROWTYPE;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    IF expected IS NULL OR COALESCE(source, '') NOT IN ('fixture', 'forge_observed')
        OR COALESCE(evidence_pin, '') !~ '^sha256:[a-f0-9]{64}$'
        OR length(btrim(COALESCE(evidence_ref, ''))) NOT BETWEEN 1 AND 500
        OR length(btrim(COALESCE(review_note, ''))) NOT BETWEEN 1 AND 1000 THEN RETURN NULL; END IF;
    SELECT r.* INTO evaluated FROM craft_queue.shadow_merge_receipts AS r
    JOIN craft_queue.attempts AS a ON a.work_id = r.work_id AND a.generation = r.generation
    WHERE r.id = receipt AND r.head = candidate_head AND r.report_digest = report_pin
        AND r.actor <> actor_id AND a.runner_id <> actor_id;
    IF NOT FOUND THEN RETURN NULL; END IF;
    SELECT * INTO outcome FROM craft_queue.shadow_outcomes AS o WHERE o.receipt_id = receipt;
    IF FOUND THEN
        IF outcome.verifier <> actor_id OR outcome.basis <> source OR outcome.expected_eligible <> expected
            OR outcome.evidence_digest <> evidence_pin OR outcome.evidence <> evidence_ref
            OR outcome.note <> review_note THEN RETURN NULL; END IF;
    ELSE
        INSERT INTO craft_queue.shadow_outcomes (receipt_id, verifier, basis, expected_eligible, evidence_digest,
            evidence, note)
        VALUES (receipt, actor_id, source, expected, evidence_pin, evidence_ref, review_note)
        RETURNING * INTO outcome;
    END IF;
    RETURN to_jsonb(outcome) || jsonb_build_object('predicted_eligible', evaluated.assessment->'eligible');
END;
$$;

CREATE FUNCTION craft_queue.shadow_qualification(repo text, since_epoch bigint) RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    WITH sample AS MATERIALIZED (
        SELECT r.policy_digest, p.require_native, r.assessment->'eligible' = 'true'::jsonb AS predicted,
            o.basis, o.expected_eligible AS expected
        FROM craft_queue.shadow_merge_receipts AS r
        JOIN craft_queue.shadow_policies AS p ON p.repository = r.repository AND p.digest = r.policy_digest
        LEFT JOIN craft_queue.shadow_outcomes AS o ON o.receipt_id = r.id
        WHERE r.repository = repo AND r.recorded_at >= to_timestamp(since_epoch::double precision)
    ), matrix AS (
        SELECT s.policy_digest, s.basis,
            count(*) FILTER (WHERE predicted AND expected) AS correct_allow,
            count(*) FILTER (WHERE NOT predicted AND NOT expected) AS correct_block,
            count(*) FILTER (WHERE predicted AND NOT expected) AS false_allow,
            count(*) FILTER (WHERE NOT predicted AND expected) AS false_block
        FROM sample AS s WHERE basis IS NOT NULL GROUP BY s.policy_digest, s.basis
    ), policies AS (
        SELECT s.policy_digest, s.require_native, count(*) AS total,
            count(*) FILTER (WHERE basis IS NULL) AS unassessed,
            COALESCE((SELECT jsonb_object_agg(m.basis, to_jsonb(m) - ARRAY['policy_digest','basis'])
                FROM matrix AS m WHERE m.policy_digest = s.policy_digest), '{}'::jsonb) AS by_basis
        FROM sample AS s GROUP BY s.policy_digest, s.require_native
    )
    SELECT COALESCE(jsonb_agg(to_jsonb(policies) ORDER BY policy_digest), '[]'::jsonb) FROM policies;
$$;

CREATE FUNCTION craft_queue.shadow_outcomes_for(order_id text) RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT COALESCE(jsonb_agg(to_jsonb(recent) ORDER BY recorded_at DESC, receipt_id), '[]'::jsonb)
    FROM (
        SELECT o.*, r.assessment->'eligible' AS predicted_eligible
        FROM craft_queue.shadow_merge_receipts AS r JOIN craft_queue.shadow_outcomes AS o ON o.receipt_id = r.id
        WHERE r.work_id = order_id ORDER BY o.recorded_at DESC, o.receipt_id LIMIT 20
    ) AS recent;
$$;

REVOKE ALL ON FUNCTION craft_queue.record_shadow_outcome(text, text, text, text, text, boolean, text, text, text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.shadow_qualification(text, bigint) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.shadow_outcomes_for(text) FROM PUBLIC;
COMMIT;
