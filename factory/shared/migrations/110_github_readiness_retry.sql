BEGIN;

CREATE TABLE craft_queue.github_readiness_attempts (
    publication_key text NOT NULL REFERENCES craft_queue.publications(action_key) ON DELETE CASCADE,
    verification_sequence bigint NOT NULL REFERENCES craft_queue.verifications(sequence) ON DELETE CASCADE,
    policy_digest text NOT NULL,
    attempted_at timestamptz NOT NULL,
    PRIMARY KEY (publication_key, verification_sequence, policy_digest)
);

CREATE OR REPLACE FUNCTION craft_queue.next_github_readiness(
    verifier_actor text, lane_repo text, lane_branch text, lane_pin text
) RETURNS jsonb LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    selected record;
    now_at timestamptz;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    IF NOT craft_queue.github_readiness_lane(verifier_actor, lane_repo, lane_branch, lane_pin) THEN
        RETURN NULL;
    END IF;
    now_at := clock_timestamp();

    SELECT w.id, w.generation, w.candidate, w.candidate_evidence,
           v.sequence, p.result_url, p.action_key INTO selected
    FROM craft_queue.work AS w
    JOIN craft_queue.attempts AS a ON a.work_id = w.id AND a.generation = w.generation
    JOIN LATERAL (
        SELECT latest.sequence FROM craft_queue.verifications AS latest
        WHERE latest.work_id = w.id AND latest.generation = w.generation
        ORDER BY latest.sequence DESC LIMIT 1
    ) AS v ON true
    JOIN craft_queue.publications AS p
      ON p.action_key = craft_queue.current_review_draft(w.id, w.generation, w.candidate, v.sequence)
    LEFT JOIN craft_queue.github_readiness_attempts AS attempt
      ON attempt.publication_key = p.action_key AND attempt.verification_sequence = v.sequence
         AND attempt.policy_digest = lane_pin
    CROSS JOIN LATERAL (
        SELECT greatest(attempt.attempted_at, max(r.recorded_at)) AS checked_at
        FROM craft_queue.shadow_merge_receipts AS r
        WHERE r.publication_key = p.action_key AND r.verification_sequence = v.sequence
          AND r.policy_digest = lane_pin
    ) AS previous
    WHERE w.repository = lane_repo AND w.state = 'candidate' AND a.runner_id <> verifier_actor
      AND p.base_branch = lane_branch
      AND p.result_url = 'https://github.com/' || lane_repo || '/pull/' || split_part(p.result_url, '/pull/', 2)
      AND split_part(p.result_url, '/pull/', 2) ~ '^[1-9][0-9]{0,9}$'
      AND (previous.checked_at IS NULL OR previous.checked_at <= now_at - interval '90 seconds')
      AND NOT EXISTS (SELECT 1 FROM craft_queue.github_enqueue_intents AS i
          WHERE i.repository = lane_repo AND i.pull_request = p.result_url AND i.head = w.candidate
            AND (i.state <> 'abandoned' OR i.send_possible))
    ORDER BY previous.checked_at NULLS FIRST, w.id COLLATE "C" LIMIT 1;
    IF NOT FOUND THEN RETURN NULL; END IF;

    INSERT INTO craft_queue.github_readiness_attempts (publication_key, verification_sequence, policy_digest, attempted_at)
    VALUES (selected.action_key, selected.sequence, lane_pin, now_at)
    ON CONFLICT (publication_key, verification_sequence, policy_digest) DO UPDATE
    SET attempted_at = EXCLUDED.attempted_at;

    RETURN jsonb_build_object('work_id', selected.id, 'generation', selected.generation,
        'head', selected.candidate, 'pack_digest', selected.candidate_evidence,
        'verification_sequence', selected.sequence, 'pull_request', selected.result_url);
END;
$$;

COMMIT;
