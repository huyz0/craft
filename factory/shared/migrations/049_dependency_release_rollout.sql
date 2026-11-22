BEGIN;

CREATE OR REPLACE FUNCTION craft_queue.register_series(
    actor text, plan_id text, plan_owner text, approval text, members jsonb
) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    fingerprint text;
    existing text;
    item record;
    order_id text;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    IF jsonb_typeof(members) IS DISTINCT FROM 'array' THEN RETURN NULL; END IF;
    IF jsonb_array_length(members) NOT BETWEEN 1 AND 12 OR COALESCE(plan_id, '') !~ '^[A-Za-z0-9_-]{1,100}$'
       OR length(btrim(COALESCE(plan_owner, ''))) NOT BETWEEN 1 AND 200
       OR length(btrim(COALESCE(approval, ''))) NOT BETWEEN 1 AND 500 THEN RETURN NULL; END IF;
    fingerprint := 'sha256:' || encode(sha256(convert_to(
        jsonb_build_array(plan_owner, approval, members)::text, 'UTF8')), 'hex');
    SELECT plan.plan_digest INTO existing FROM craft_queue.release_series AS plan WHERE plan.id = plan_id;
    IF FOUND THEN
        IF existing = fingerprint THEN RETURN craft_queue.series_status(plan_id); END IF;
        RETURN NULL;
    END IF;
    IF (jsonb_array_length(members) = 1 AND NOT COALESCE((members->0->'work_order') ? 'dependency_intake_digest', false))
        OR (jsonb_array_length(members) > 1 AND
            (SELECT count(DISTINCT value->>'repository') FROM jsonb_array_elements(members)) < 2)
        OR EXISTS (SELECT 1 FROM jsonb_array_elements(members) AS m(member)
            WHERE (m.member->'work_order') ? 'dependency_intake_digest'
               AND m.member->'requires_rollout_proof' IS DISTINCT FROM 'true'::jsonb) THEN
        RETURN NULL;
    END IF;
    INSERT INTO craft_queue.release_series(id, owner_name, approval_reference, registered_by, plan_digest)
    VALUES (plan_id, plan_owner, approval, actor, fingerprint);
    FOR item IN SELECT value AS member, ordinality AS ordinal FROM jsonb_array_elements(members) WITH ORDINALITY
    LOOP
        order_id := item.member #>> '{work_order,id}';
        IF COALESCE(order_id, '') !~ '^[A-Za-z0-9_-]{1,100}$'
           OR EXISTS (SELECT 1 FROM craft_queue.work AS w WHERE w.id = order_id)
           OR length(btrim(COALESCE(item.member #>> '{work_order,owner}', ''))) NOT BETWEEN 1 AND 200
           OR COALESCE(item.member->>'observer', '') !~ '^[A-Za-z0-9_-]{1,100}$' THEN
            RAISE EXCEPTION 'series requires new work IDs, owners, and release observers';
        END IF;
        PERFORM craft_queue.admit(order_id, item.member->>'repository',
            'series:' || plan_id || ':' || item.ordinal::text, item.member->'work_order',
            (item.member->>'token_limit')::bigint);
        INSERT INTO craft_queue.series_members(
            series_id, position, work_id, observer_id, release_target, requires_rollout_proof
        ) VALUES (plan_id, item.ordinal, order_id, item.member->>'observer', item.member->'target',
            COALESCE((item.member->>'requires_rollout_proof')::boolean, false));
    END LOOP;
    RETURN craft_queue.series_status(plan_id);
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
      ))
      AND (merge_binding.observation IS NOT NULL OR (p.action_key IS NULL AND policy.forge IS DISTINCT FROM 'github'));
$$;

UPDATE craft_queue.series_members AS member SET requires_rollout_proof = true
FROM craft_queue.work AS w
WHERE w.id = member.work_id AND w.state <> 'released' AND w.payload ? 'dependency_intake_digest';

COMMIT;
