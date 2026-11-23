BEGIN;

ALTER TABLE craft_queue.series_observations ADD COLUMN github_merge jsonb;

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
      AND (merge_binding.observation IS NOT NULL OR (p.action_key IS NULL AND policy.forge IS DISTINCT FROM 'github'));
$$;

CREATE FUNCTION craft_queue.record_series_verified_rollout(
    actor_id text, order_id text, candidate_generation bigint, candidate_head text, review_sequence bigint,
    policy_digest text, evidence_digest text, artifact_digest text, fresh_through bigint, index_digest text,
    verified_github_merge jsonb
) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    binding jsonb;
    result jsonb;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    binding := craft_queue.series_rollout_binding(actor_id, order_id, candidate_generation, candidate_head, review_sequence);
    IF binding IS NULL OR binding->'github_merge' IS DISTINCT FROM COALESCE(verified_github_merge, 'null'::jsonb)
        THEN RETURN NULL; END IF;
    result := craft_queue.record_series_verified_rollout(actor_id, order_id, candidate_generation, candidate_head,
        review_sequence, policy_digest, evidence_digest, artifact_digest, fresh_through, index_digest);
    IF result IS NULL OR verified_github_merge IS NULL THEN RETURN result; END IF;
    UPDATE craft_queue.series_observations AS o SET github_merge = verified_github_merge
    WHERE o.sequence = (result->>'sequence')::bigint
        AND (o.github_merge IS NULL OR o.github_merge = verified_github_merge)
    RETURNING to_jsonb(o) INTO result;
    RETURN result;
END;
$$;

CREATE FUNCTION craft_queue.record_rollout_refresh(
    actor_id text, order_id text, candidate_generation bigint, candidate_head text, review_sequence bigint,
    refresh_id text, policy_digest text, index_digest text, artifact_digest text, fresh_through bigint,
    verified_github_merge jsonb
) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    binding jsonb;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    binding := craft_queue.series_rollout_refresh_binding(
        actor_id, order_id, candidate_generation, candidate_head, review_sequence, refresh_id);
    IF binding IS NULL OR binding->'github_merge' IS DISTINCT FROM COALESCE(verified_github_merge, 'null'::jsonb)
        THEN RETURN NULL; END IF;
    IF NOT EXISTS (
        SELECT 1 FROM craft_queue.series_rollout_refreshes AS r
        JOIN craft_queue.series_observations AS o ON o.sequence = r.observation_sequence
        WHERE r.id = refresh_id AND o.github_merge IS NOT DISTINCT FROM verified_github_merge
    ) THEN RETURN NULL; END IF;
    RETURN craft_queue.record_rollout_refresh(actor_id, order_id, candidate_generation, candidate_head,
        review_sequence, refresh_id, policy_digest, index_digest, artifact_digest, fresh_through);
END;
$$;

REVOKE ALL ON FUNCTION craft_queue.record_series_verified_rollout(text, text, bigint, text, bigint, text, text, text, bigint, text, jsonb) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.record_rollout_refresh(text, text, bigint, text, bigint, text, text, text, text, bigint, jsonb) FROM PUBLIC;
COMMIT;
