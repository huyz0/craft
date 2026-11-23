BEGIN;

ALTER TABLE craft_queue.series_rollout_approvals ADD COLUMN archive_index_digest text
    CHECK (archive_index_digest IS NULL OR archive_index_digest ~ '^sha256:[a-f0-9]{64}$');

CREATE FUNCTION craft_queue.approve_series_rollout(
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
       OR (index_digest IS NOT NULL AND index_digest !~ '^sha256:[a-f0-9]{64}$') THEN RETURN NULL; END IF;
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
    ) OR craft_queue.series_blocker(order_id) IS NOT NULL THEN RETURN NULL; END IF;
    INSERT INTO craft_queue.series_rollout_approvals (
        work_id, generation, verification_sequence, head, policy_digest, actor, approval_reference, archive_index_digest
    ) VALUES (order_id, candidate_generation, review_sequence, candidate_head, digest, operator_id, approval_ref, index_digest)
    RETURNING * INTO existing;
    RETURN to_jsonb(existing);
END;
$$;

CREATE OR REPLACE FUNCTION craft_queue.approve_series_rollout(
    operator_id text, order_id text, candidate_generation bigint, candidate_head text,
    review_sequence bigint, digest text, approval_ref text
) RETURNS jsonb
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT craft_queue.approve_series_rollout(operator_id, order_id, candidate_generation, candidate_head,
        review_sequence, digest, approval_ref, NULL::text);
$$;

CREATE OR REPLACE FUNCTION craft_queue.series_rollout_binding(
    actor_id text, order_id text, candidate_generation bigint, candidate_head text, review_sequence bigint
) RETURNS jsonb
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT jsonb_build_object(
        'repository', w.repository, 'work_item', w.payload->>'issue', 'target', member.release_target,
        'policy_digest', approval.policy_digest, 'archive_index_digest', approval.archive_index_digest,
        'as_of', floor(extract(epoch FROM statement_timestamp()))::bigint
    )
    FROM craft_queue.series_rollout_approvals AS approval
    JOIN craft_queue.series_members AS member ON member.work_id = approval.work_id
    JOIN craft_queue.work AS w ON w.id = member.work_id
    WHERE member.observer_id = actor_id AND member.requires_rollout_proof AND w.id = order_id
      AND w.state IN ('candidate', 'released') AND w.generation = candidate_generation AND w.candidate = candidate_head
      AND approval.generation = candidate_generation AND approval.head = candidate_head
      AND approval.verification_sequence = review_sequence;
$$;

CREATE FUNCTION craft_queue.record_series_verified_rollout(
    actor_id text, order_id text, candidate_generation bigint, candidate_head text,
    review_sequence bigint, policy_digest text, evidence_digest text, artifact_digest text,
    fresh_through bigint, index_digest text
) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    binding jsonb;
    result jsonb;
    observed craft_queue.series_observations%ROWTYPE;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    binding := craft_queue.series_rollout_binding(actor_id, order_id, candidate_generation, candidate_head, review_sequence);
    IF binding IS NULL OR binding->>'policy_digest' IS DISTINCT FROM policy_digest
       OR binding->>'archive_index_digest' IS DISTINCT FROM index_digest
       OR (index_digest IS NOT NULL AND index_digest IS DISTINCT FROM evidence_digest)
       OR COALESCE(evidence_digest, '') !~ '^sha256:[a-f0-9]{64}$'
       OR fresh_through IS NULL OR floor(extract(epoch FROM clock_timestamp()))::bigint > fresh_through
       OR craft_queue.series_blocker(order_id) IS NOT NULL THEN RETURN NULL; END IF;
    result := craft_queue.record_series_release_receipt(actor_id, order_id, candidate_generation, candidate_head,
        review_sequence, binding->'target', artifact_digest, 'rollout:' || evidence_digest);
    IF result IS NULL THEN RETURN NULL; END IF;
    UPDATE craft_queue.series_observations AS o
    SET rollout_policy_digest = policy_digest, rollout_evidence_digest = evidence_digest
    WHERE o.sequence = (result->>'sequence')::bigint
    RETURNING o.* INTO observed;
    RETURN to_jsonb(observed);
END;
$$;

CREATE OR REPLACE FUNCTION craft_queue.record_series_verified_rollout(
    actor_id text, order_id text, candidate_generation bigint, candidate_head text,
    review_sequence bigint, policy_digest text, evidence_digest text, artifact_digest text, fresh_through bigint
) RETURNS jsonb
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT craft_queue.record_series_verified_rollout(actor_id, order_id, candidate_generation, candidate_head,
        review_sequence, policy_digest, evidence_digest, artifact_digest, fresh_through, NULL::text);
$$;

REVOKE ALL ON FUNCTION craft_queue.approve_series_rollout(text, text, bigint, text, bigint, text, text, text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.record_series_verified_rollout(text, text, bigint, text, bigint, text, text, text, bigint, text) FROM PUBLIC;

COMMIT;
