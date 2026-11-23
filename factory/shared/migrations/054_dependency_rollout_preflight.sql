BEGIN;

CREATE FUNCTION craft_queue.series_dependency_scan_expired(
    actor_id text, order_id text, candidate_generation bigint, candidate_head text, review_sequence bigint
) RETURNS boolean
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT EXISTS (
        SELECT 1
        FROM craft_queue.work AS w
        JOIN craft_queue.series_members AS member ON member.work_id = w.id
        JOIN craft_queue.series_rollout_approvals AS approval ON approval.work_id = w.id
        JOIN LATERAL (
            SELECT v.* FROM craft_queue.verifications AS v
            WHERE v.work_id = w.id AND v.generation = w.generation ORDER BY v.sequence DESC LIMIT 1
        ) AS latest ON true
        JOIN craft_queue.dependency_verifications AS checked ON checked.verification_sequence = latest.sequence
        WHERE w.id = order_id AND w.state = 'candidate' AND w.payload ? 'dependency_intake_digest'
          AND w.generation = candidate_generation AND w.candidate = candidate_head
          AND member.observer_id = actor_id AND member.requires_rollout_proof
          AND approval.generation = w.generation AND approval.head = w.candidate
          AND approval.verification_sequence = review_sequence AND approval.archive_index_digest IS NOT NULL
          AND latest.sequence = review_sequence AND latest.passed AND latest.head = w.candidate
          AND latest.digest = w.candidate_evidence AND checked.approval_id IS NOT NULL
          AND CASE WHEN jsonb_typeof(checked.proof->'scanner_fresh_through') = 'number'
                        AND checked.proof->>'scanner_fresh_through' ~ '^[1-9][0-9]{0,11}$'
                   THEN (checked.proof->>'scanner_fresh_through')::bigint
                        < floor(extract(epoch FROM statement_timestamp()))::bigint
                   ELSE false END
    );
$$;

REVOKE ALL ON FUNCTION craft_queue.series_dependency_scan_expired(text,text,bigint,text,bigint) FROM PUBLIC;

COMMIT;
