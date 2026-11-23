BEGIN;

ALTER TABLE craft_queue.series_observations ADD COLUMN rollout_fresh_through bigint
    CHECK (rollout_fresh_through IS NULL OR (rollout_fresh_through > 0 AND rollout_policy_digest IS NOT NULL));

-- Existing archive receipts kept the time of recording but did not retain their freshness bound.
UPDATE craft_queue.series_observations AS observed
SET rollout_fresh_through = floor(extract(epoch FROM observed.observed_at))::bigint
FROM craft_queue.series_rollout_approvals AS approval
WHERE approval.work_id = observed.work_id AND approval.generation = observed.generation
  AND approval.verification_sequence = observed.verification_sequence
  AND approval.archive_index_digest = observed.rollout_evidence_digest;

CREATE OR REPLACE FUNCTION craft_queue.series_release_current(order_id text) RETURNS boolean
LANGUAGE sql STABLE SET search_path = pg_catalog AS $$
    WITH cutoff AS (SELECT floor(extract(epoch FROM statement_timestamp()))::bigint AS seconds)
    SELECT EXISTS (
        SELECT 1 FROM craft_queue.series_members AS member
        JOIN craft_queue.work AS w ON w.id = member.work_id AND w.state = 'released'
        JOIN craft_queue.series_observations AS observed ON observed.work_id = w.id AND observed.withdrawn_at IS NULL
        CROSS JOIN cutoff
        WHERE member.work_id = order_id
          AND (observed.rollout_fresh_through IS NULL OR observed.rollout_fresh_through >= cutoff.seconds)
          AND NOT EXISTS (
            SELECT 1 FROM jsonb_each(member.predecessor_releases) AS pin
            LEFT JOIN craft_queue.series_observations AS prior ON prior.work_id = pin.key AND prior.withdrawn_at IS NULL
            WHERE prior.sequence IS NULL OR prior.rollout_fresh_through < cutoff.seconds OR pin.value IS DISTINCT FROM
                jsonb_build_object('head', prior.head, 'artifact_digest', prior.artifact_digest)
        )
    );
$$;

CREATE OR REPLACE FUNCTION craft_queue.series_status(plan_id text) RETURNS jsonb
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT to_jsonb(plan) || jsonb_build_object(
        'state', CASE WHEN summary.released = summary.total THEN 'released'
                      WHEN summary.released > 0 THEN 'partial' ELSE 'open' END,
        'released_members', summary.released, 'total_members', summary.total, 'members', summary.members
    )
    FROM craft_queue.release_series AS plan
    CROSS JOIN LATERAL (
        SELECT count(*) AS total, count(*) FILTER (WHERE release_state.release_current) AS released,
               jsonb_agg(jsonb_build_object(
                   'position', member.position, 'work_id', w.id, 'repository', w.repository,
                   'owner', w.payload->>'owner', 'state', w.state, 'generation', w.generation,
                   'head', w.candidate, 'waiting_on', craft_queue.series_blocker(w.id),
                   'target', member.release_target, 'observer', member.observer_id,
                   'predecessor_releases', member.predecessor_releases,
                   'requires_rollout_proof', member.requires_rollout_proof, 'release_current', release_state.release_current,
                   'latest_rollout_approval', (SELECT to_jsonb(a) FROM craft_queue.series_rollout_approvals AS a
                        WHERE a.work_id = w.id AND a.generation = w.generation AND a.head = w.candidate
                        ORDER BY a.verification_sequence DESC LIMIT 1),
                   'latest_release', (SELECT to_jsonb(o) FROM craft_queue.series_observations AS o
                        WHERE o.work_id = w.id ORDER BY o.sequence DESC LIMIT 1)
               ) ORDER BY member.position) AS members
        FROM craft_queue.series_members AS member JOIN craft_queue.work AS w ON w.id = member.work_id
        CROSS JOIN LATERAL (SELECT craft_queue.series_release_current(w.id) AS release_current) AS release_state
        WHERE member.series_id = plan.id
    ) AS summary
    WHERE plan.id = plan_id;
$$;

CREATE OR REPLACE FUNCTION craft_queue.record_series_verified_rollout(
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
    SET rollout_policy_digest = policy_digest, rollout_evidence_digest = evidence_digest,
        rollout_fresh_through = CASE WHEN index_digest IS NOT NULL THEN fresh_through END
    WHERE o.sequence = (result->>'sequence')::bigint
      AND (o.rollout_fresh_through IS NULL OR o.rollout_fresh_through = fresh_through)
    RETURNING o.* INTO observed;
    IF NOT FOUND THEN RETURN NULL; END IF;
    RETURN to_jsonb(observed);
END;
$$;

COMMIT;
