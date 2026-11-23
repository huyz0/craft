BEGIN;

CREATE FUNCTION craft_queue.release_observer_status(actor_id text, order_id text) RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT jsonb_build_object(
        'work_id', w.id, 'repository', w.repository, 'target', member.release_target,
        'generation', w.generation, 'head', w.candidate, 'state', w.state,
        'requires_rollout_proof', member.requires_rollout_proof,
        'verification_sequence', CASE WHEN verification.current THEN verdict.sequence END,
        'policy_digest', approval.policy_digest, 'archive_index_digest', approval.archive_index_digest,
        'release_current', blocker.release_current,
        'latest_release', CASE WHEN observation.sequence IS NOT NULL THEN jsonb_build_object(
            'sequence', observation.sequence, 'artifact_digest', observation.artifact_digest,
            'observed_at', observation.observed_at, 'withdrawn_at', observation.withdrawn_at,
            'rollout_fresh_through', craft_queue.series_observation_fresh_through(
                observation.sequence, observation.rollout_fresh_through
            )
        ) END,
        'next_action', CASE
            WHEN blocker.release_current THEN 'released'
            WHEN w.state = 'cancelled' THEN 'cancelled'
            WHEN blocker.predecessor IS NOT NULL AND craft_queue.series_release_current(blocker.predecessor)
                THEN 'prerequisite_changed'
            WHEN blocker.predecessor IS NOT NULL THEN 'prerequisite_not_current'
            WHEN EXISTS (SELECT 1 FROM craft_queue.recoveries WHERE finished_at IS NULL) THEN 'wait_for_recovery'
            WHEN w.state = 'released' THEN 'refresh_rollout_evidence'
            WHEN w.state <> 'candidate' THEN 'wait_for_candidate'
            WHEN NOT verification.current THEN 'wait_for_verification'
            WHEN EXISTS (SELECT 1 FROM craft_queue.publications AS p WHERE p.work_id = w.id
                AND p.state IN ('reserved','sending','uncertain')) THEN 'reconcile_publication'
            WHEN NOT craft_queue.team_skill_allowed(w.repository, w.payload)
                OR NOT craft_queue.project_context_allowed(w.repository, w.payload) THEN 'review_candidate_context'
            WHEN member.requires_rollout_proof AND approval.policy_digest IS NULL THEN 'wait_for_rollout_approval'
            WHEN member.requires_rollout_proof THEN 'check_rollout_evidence'
            ELSE 'submit_observation'
        END
    )
    FROM craft_queue.series_members AS member
    JOIN craft_queue.work AS w ON w.id = member.work_id
    CROSS JOIN LATERAL (
        SELECT craft_queue.series_blocker(w.id) AS predecessor,
               craft_queue.series_release_current(w.id) AS release_current
    ) AS blocker
    LEFT JOIN LATERAL (
        SELECT v.sequence, v.passed, v.head, v.digest FROM craft_queue.verifications AS v
        WHERE v.work_id = w.id AND v.generation = w.generation ORDER BY v.sequence DESC LIMIT 1
    ) AS verdict ON true
    CROSS JOIN LATERAL (
        SELECT COALESCE(verdict.passed AND verdict.head = w.candidate AND verdict.digest = w.candidate_evidence, false) AS current
    ) AS verification
    LEFT JOIN craft_queue.series_rollout_approvals AS approval ON verification.current
        AND approval.work_id = w.id AND approval.generation = w.generation
        AND approval.head = w.candidate AND approval.verification_sequence = verdict.sequence
    LEFT JOIN LATERAL (
        SELECT o.* FROM craft_queue.series_observations AS o
        WHERE o.work_id = w.id ORDER BY o.sequence DESC LIMIT 1
    ) AS observation ON true
    WHERE member.work_id = order_id AND member.observer_id = actor_id;
$$;

REVOKE ALL ON FUNCTION craft_queue.release_observer_status(text,text) FROM PUBLIC;

COMMIT;
