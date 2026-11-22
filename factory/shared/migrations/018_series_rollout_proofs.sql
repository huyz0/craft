BEGIN;

ALTER TABLE craft_queue.series_members ADD COLUMN requires_rollout_proof boolean NOT NULL DEFAULT false;
ALTER TABLE craft_queue.series_observations
    ADD COLUMN rollout_policy_digest text,
    ADD COLUMN rollout_evidence_digest text,
    ADD CONSTRAINT series_rollout_digests_paired CHECK (
        (rollout_policy_digest IS NULL AND rollout_evidence_digest IS NULL)
        OR (rollout_policy_digest ~ '^sha256:[a-f0-9]{64}$'
            AND rollout_evidence_digest ~ '^sha256:[a-f0-9]{64}$'
            AND rollout_policy_digest IS NOT NULL AND rollout_evidence_digest IS NOT NULL)
    );

CREATE TABLE craft_queue.series_rollout_approvals (
    work_id text NOT NULL REFERENCES craft_queue.series_members(work_id),
    generation bigint NOT NULL,
    verification_sequence bigint NOT NULL REFERENCES craft_queue.verifications(sequence),
    head text NOT NULL,
    policy_digest text NOT NULL CHECK (policy_digest ~ '^sha256:[a-f0-9]{64}$'),
    actor text NOT NULL,
    approval_reference text NOT NULL CHECK (length(btrim(approval_reference)) BETWEEN 1 AND 500),
    approved_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (work_id, generation, verification_sequence)
);

CREATE OR REPLACE FUNCTION craft_queue.series_status(plan_id text) RETURNS jsonb
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT to_jsonb(plan) || jsonb_build_object(
        'state', CASE WHEN summary.released = summary.total THEN 'released'
                      WHEN summary.released > 0 THEN 'partial' ELSE 'open' END,
        'released_members', summary.released, 'total_members', summary.total, 'members', summary.members
    )
    FROM craft_queue.release_series AS plan
    CROSS JOIN LATERAL (
        SELECT count(*) AS total, count(*) FILTER (WHERE craft_queue.series_release_current(w.id)) AS released,
               jsonb_agg(jsonb_build_object(
                   'position', member.position, 'work_id', w.id, 'repository', w.repository,
                   'owner', w.payload->>'owner', 'state', w.state, 'generation', w.generation,
                   'head', w.candidate, 'waiting_on', craft_queue.series_blocker(w.id),
                   'target', member.release_target, 'observer', member.observer_id,
                   'predecessor_releases', member.predecessor_releases,
                   'requires_rollout_proof', member.requires_rollout_proof,
                   'latest_rollout_approval', (SELECT to_jsonb(a) FROM craft_queue.series_rollout_approvals AS a
                        WHERE a.work_id = w.id AND a.generation = w.generation AND a.head = w.candidate
                        ORDER BY a.verification_sequence DESC LIMIT 1),
                   'latest_release', (SELECT to_jsonb(o) FROM craft_queue.series_observations AS o
                        WHERE o.work_id = w.id ORDER BY o.sequence DESC LIMIT 1)
               ) ORDER BY member.position) AS members
        FROM craft_queue.series_members AS member JOIN craft_queue.work AS w ON w.id = member.work_id
        WHERE member.series_id = plan.id
    ) AS summary
    WHERE plan.id = plan_id;
$$;

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
    IF jsonb_array_length(members) NOT BETWEEN 2 AND 12 OR COALESCE(plan_id, '') !~ '^[A-Za-z0-9_-]{1,100}$'
       OR length(btrim(COALESCE(plan_owner, ''))) NOT BETWEEN 1 AND 200
       OR length(btrim(COALESCE(approval, ''))) NOT BETWEEN 1 AND 500 THEN RETURN NULL; END IF;
    fingerprint := 'sha256:' || encode(sha256(convert_to(
        jsonb_build_array(plan_owner, approval, members)::text, 'UTF8')), 'hex');
    SELECT plan.plan_digest INTO existing FROM craft_queue.release_series AS plan WHERE plan.id = plan_id;
    IF FOUND THEN
        IF existing = fingerprint THEN RETURN craft_queue.series_status(plan_id); END IF;
        RETURN NULL;
    END IF;
    IF (SELECT count(DISTINCT value->>'repository') FROM jsonb_array_elements(members)) < 2 THEN
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

CREATE FUNCTION craft_queue.approve_series_rollout(
    operator_id text, order_id text, candidate_generation bigint, candidate_head text,
    review_sequence bigint, digest text, approval_ref text
) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    existing craft_queue.series_rollout_approvals%ROWTYPE;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    IF COALESCE(digest, '') !~ '^sha256:[a-f0-9]{64}$'
       OR length(btrim(COALESCE(approval_ref, ''))) NOT BETWEEN 1 AND 500 THEN RETURN NULL; END IF;
    SELECT approval.* INTO existing FROM craft_queue.series_rollout_approvals AS approval
    WHERE approval.work_id = order_id AND approval.generation = candidate_generation
      AND approval.verification_sequence = review_sequence;
    IF FOUND THEN
        IF existing.head = candidate_head AND existing.policy_digest = digest AND existing.actor = operator_id
           AND existing.approval_reference = approval_ref THEN RETURN to_jsonb(existing); END IF;
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
        work_id, generation, verification_sequence, head, policy_digest, actor, approval_reference
    ) VALUES (order_id, candidate_generation, review_sequence, candidate_head, digest, operator_id, approval_ref)
    RETURNING * INTO existing;
    RETURN to_jsonb(existing);
END;
$$;

CREATE FUNCTION craft_queue.series_rollout_binding(
    actor_id text, order_id text, candidate_generation bigint, candidate_head text, review_sequence bigint
) RETURNS jsonb
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT jsonb_build_object(
        'repository', w.repository, 'work_item', w.payload->>'issue', 'target', member.release_target,
        'policy_digest', approval.policy_digest,
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

ALTER FUNCTION craft_queue.record_series_release(text, text, bigint, text, bigint, jsonb, text, text)
    RENAME TO record_series_release_receipt;

CREATE FUNCTION craft_queue.record_series_release(
    actor text, order_id text, candidate_generation bigint, candidate_head text,
    review_sequence bigint, observed_target jsonb, digest text, evidence_ref text
) RETURNS jsonb
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT craft_queue.record_series_release_receipt(
        actor, order_id, candidate_generation, candidate_head, review_sequence, observed_target, digest, evidence_ref
    )
    FROM craft_queue.series_members AS member
    WHERE member.work_id = order_id AND NOT member.requires_rollout_proof;
$$;

CREATE FUNCTION craft_queue.record_series_verified_rollout(
    actor_id text, order_id text, candidate_generation bigint, candidate_head text,
    review_sequence bigint, policy_digest text, evidence_digest text, artifact_digest text, fresh_through bigint
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

REVOKE ALL ON FUNCTION craft_queue.approve_series_rollout(text, text, bigint, text, bigint, text, text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.series_rollout_binding(text, text, bigint, text, bigint) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.record_series_release(text, text, bigint, text, bigint, jsonb, text, text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.record_series_verified_rollout(text, text, bigint, text, bigint, text, text, text, bigint) FROM PUBLIC;
COMMIT;
