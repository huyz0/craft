BEGIN;

CREATE FUNCTION craft_queue.dependency_scan_fresh(pins jsonb, checked_at timestamptz) RETURNS boolean
LANGUAGE sql STABLE SET search_path = pg_catalog AS $$
    SELECT CASE
        WHEN NOT (pins ? 'scanner_fresh_through') THEN true
        WHEN jsonb_typeof(pins->'scanner_fresh_through') = 'number'
            AND pins->>'scanner_fresh_through' ~ '^[1-9][0-9]{0,11}$'
        THEN (pins->>'scanner_fresh_through')::bigint BETWEEN
            floor(extract(epoch FROM checked_at))::bigint
            AND floor(extract(epoch FROM checked_at))::bigint + 604800
        ELSE false
    END;
$$;
REVOKE ALL ON FUNCTION craft_queue.dependency_scan_fresh(jsonb, timestamptz) FROM PUBLIC;

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
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    IF COALESCE(approval_id, '') !~ '^[A-Za-z0-9_-]{1,100}$'
        OR length(btrim(COALESCE(approval_ref, ''))) NOT BETWEEN 1 AND 500
        OR EXISTS (SELECT 1 FROM unnest(ARRAY['intake_digest','policy_digest',
            'baseline_run_digest','candidate_run_digest']) AS fields(name)
            WHERE COALESCE(evidence_pins->>fields.name, '') !~ '^sha256:[a-f0-9]{64}$')
        OR evidence_pins->>'baseline_run_digest' = evidence_pins->>'candidate_run_digest'
        OR NOT craft_queue.dependency_scan_fresh(evidence_pins, clock_timestamp())
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

CREATE OR REPLACE FUNCTION craft_queue.record_dependency_verification(
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
        OR NOT craft_queue.dependency_scan_fresh(active.pins, clock_timestamp())
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

CREATE OR REPLACE FUNCTION craft_queue.approve_series_rollout(
    operator_id text, order_id text, candidate_generation bigint, candidate_head text,
    review_sequence bigint, digest text, approval_ref text, index_digest text
) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    existing craft_queue.series_rollout_approvals%ROWTYPE;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    IF COALESCE(digest, '') !~ '^sha256:[a-f0-9]{64}$'
       OR length(btrim(COALESCE(approval_ref, ''))) NOT BETWEEN 1 AND 500
       OR (index_digest IS NOT NULL AND index_digest !~ '^sha256:[a-f0-9]{64}$')
       OR (index_digest IS NULL AND EXISTS (SELECT 1 FROM craft_queue.work AS w
           WHERE w.id = order_id AND w.payload ? 'dependency_intake_digest')) THEN RETURN NULL; END IF;
    SELECT approval.* INTO existing FROM craft_queue.series_rollout_approvals AS approval
    WHERE approval.work_id = order_id AND approval.generation = candidate_generation
      AND approval.verification_sequence = review_sequence;
    IF FOUND THEN
        IF existing.head = candidate_head AND existing.policy_digest = digest AND existing.actor = operator_id
           AND existing.approval_reference = approval_ref
           AND existing.archive_index_digest IS NOT DISTINCT FROM index_digest THEN RETURN to_jsonb(existing); END IF;
        RETURN NULL;
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM craft_queue.series_members AS member
        JOIN craft_queue.work AS w ON w.id = member.work_id
        JOIN LATERAL (
            SELECT v.* FROM craft_queue.verifications AS v
            WHERE v.work_id = w.id AND v.generation = w.generation ORDER BY v.sequence DESC LIMIT 1
        ) AS latest ON true
        WHERE w.id = order_id AND member.requires_rollout_proof AND w.state = 'candidate'
          AND w.generation = candidate_generation AND w.candidate = candidate_head
          AND latest.sequence = review_sequence AND latest.passed AND latest.head = candidate_head
          AND latest.digest = w.candidate_evidence
          AND (NOT (w.payload ? 'dependency_intake_digest') OR EXISTS (
              SELECT 1 FROM craft_queue.dependency_verifications AS checked
              WHERE checked.verification_sequence = latest.sequence AND checked.approval_id IS NOT NULL
                  AND craft_queue.dependency_scan_fresh(checked.proof, clock_timestamp())
          ))
    ) OR craft_queue.series_blocker(order_id) IS NOT NULL THEN RETURN NULL; END IF;
    INSERT INTO craft_queue.series_rollout_approvals (
        work_id, generation, verification_sequence, head, policy_digest, actor, approval_reference, archive_index_digest
    ) VALUES (order_id, candidate_generation, review_sequence, candidate_head, digest, operator_id, approval_ref, index_digest)
    RETURNING * INTO existing;
    RETURN to_jsonb(existing);
END;
$$;

CREATE OR REPLACE FUNCTION craft_queue.series_rollout_binding(
    actor_id text, order_id text, candidate_generation bigint, candidate_head text, review_sequence bigint
) RETURNS jsonb
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT jsonb_build_object(
        'repository', w.repository, 'work_item', w.payload->>'issue', 'target', member.release_target,
        'policy_digest', approval.policy_digest, 'archive_index_digest', approval.archive_index_digest,
        'github_merge', merge_binding.observation,
        'as_of', floor(extract(epoch FROM statement_timestamp()))::bigint
    )
    FROM craft_queue.series_rollout_approvals AS approval
    JOIN craft_queue.series_members AS member ON member.work_id = approval.work_id
    JOIN craft_queue.work AS w ON w.id = member.work_id
    LEFT JOIN craft_queue.draft_policies AS policy ON policy.repository = w.repository
    LEFT JOIN craft_queue.publications AS p ON p.work_id = w.id AND p.generation = w.generation
        AND p.head = w.candidate AND p.pack_digest = w.candidate_evidence AND p.state = 'confirmed'
        AND p.result_url LIKE 'https://github.com/%'
    LEFT JOIN LATERAL (
        SELECT jsonb_build_object('pull_request', p.result_url,
            'result_commit', min(m.observation->>'result_commit')) AS observation
        FROM craft_queue.shadow_merge_receipts AS r
        JOIN craft_queue.github_shadow_merges AS m ON m.receipt_id = r.id
        WHERE approval.archive_index_digest IS NOT NULL AND r.publication_key = p.action_key
            AND r.work_id = w.id AND r.generation = w.generation
            AND r.verification_sequence = review_sequence AND r.head = w.candidate
            AND r.pack_digest = w.candidate_evidence
        HAVING count(DISTINCT m.observation->>'result_commit') = 1
    ) AS merge_binding ON TRUE
    WHERE member.observer_id = actor_id AND member.requires_rollout_proof AND w.id = order_id
      AND w.state IN ('candidate', 'released') AND w.generation = candidate_generation AND w.candidate = candidate_head
      AND approval.generation = candidate_generation AND approval.head = candidate_head
      AND approval.verification_sequence = review_sequence
      AND (NOT (w.payload ? 'dependency_intake_digest') OR (
          approval.archive_index_digest IS NOT NULL AND review_sequence = (
              SELECT v.sequence FROM craft_queue.verifications AS v
              WHERE v.work_id = w.id AND v.generation = w.generation
              ORDER BY v.sequence DESC LIMIT 1
          )
          AND (w.state = 'released' OR EXISTS (
              SELECT 1 FROM craft_queue.dependency_verifications AS checked
              WHERE checked.verification_sequence = review_sequence AND checked.approval_id IS NOT NULL
                  AND craft_queue.dependency_scan_fresh(checked.proof, clock_timestamp())
          ))
      ))
      AND (merge_binding.observation IS NOT NULL OR (p.action_key IS NULL AND policy.forge IS DISTINCT FROM 'github'));
$$;

COMMIT;
