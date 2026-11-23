BEGIN;

CREATE FUNCTION craft_queue.github_readiness_lane(
    verifier_actor text, lane_repo text, lane_branch text, lane_pin text
) RETURNS boolean LANGUAGE sql STABLE SET search_path = pg_catalog AS $$
    SELECT EXISTS (SELECT 1 FROM craft_queue.github_enqueue_policies AS p
        JOIN craft_queue.shadow_policies AS s ON s.repository = p.repository AND s.digest = lane_pin
        WHERE p.repository = lane_repo AND p.base_branch = lane_branch
          AND p.policy->>'shadow_policy_digest' = lane_pin AND p.policy->>'publisher' <> verifier_actor
          AND p.revoked_at IS NULL AND p.expires_at > statement_timestamp()
          AND s.require_native AND s.expires_at > statement_timestamp())
        AND NOT EXISTS (SELECT 1 FROM craft_queue.control WHERE paused)
        AND NOT EXISTS (SELECT 1 FROM craft_queue.recoveries WHERE finished_at IS NULL);
$$;

CREATE FUNCTION craft_queue.next_github_readiness(
    verifier_actor text, lane_repo text, lane_branch text, lane_pin text
) RETURNS jsonb LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT jsonb_build_object('work_id', w.id, 'generation', w.generation,
        'head', w.candidate, 'pack_digest', w.candidate_evidence,
        'verification_sequence', v.sequence, 'pull_request', p.result_url)
    FROM craft_queue.work AS w
    JOIN craft_queue.attempts AS a ON a.work_id = w.id AND a.generation = w.generation
    JOIN LATERAL (
        SELECT latest.sequence FROM craft_queue.verifications AS latest
        WHERE latest.work_id = w.id AND latest.generation = w.generation
        ORDER BY latest.sequence DESC LIMIT 1
    ) AS v ON true
    JOIN craft_queue.publications AS p
      ON p.action_key = craft_queue.current_review_draft(w.id, w.generation, w.candidate, v.sequence)
    LEFT JOIN LATERAL (
        SELECT max(r.recorded_at) AS checked_at FROM craft_queue.shadow_merge_receipts AS r
        WHERE r.publication_key = p.action_key AND r.verification_sequence = v.sequence
          AND r.policy_digest = lane_pin
    ) AS previous ON true
    WHERE craft_queue.github_readiness_lane(verifier_actor, lane_repo, lane_branch, lane_pin)
      AND w.repository = lane_repo AND w.state = 'candidate' AND a.runner_id <> verifier_actor
      AND p.base_branch = lane_branch
      AND p.result_url = 'https://github.com/' || lane_repo || '/pull/' || split_part(p.result_url, '/pull/', 2)
      AND split_part(p.result_url, '/pull/', 2) ~ '^[1-9][0-9]{0,9}$'
      AND (previous.checked_at IS NULL OR previous.checked_at <= statement_timestamp() - interval '90 seconds')
      AND NOT EXISTS (SELECT 1 FROM craft_queue.github_enqueue_intents AS i
          WHERE i.repository = lane_repo AND i.pull_request = p.result_url AND i.head = w.candidate
            AND (i.state <> 'abandoned' OR i.send_possible))
    ORDER BY previous.checked_at NULLS FIRST, w.id COLLATE "C" LIMIT 1;
$$;

CREATE FUNCTION craft_queue.record_github_readiness(
    verifier_actor text, receipt_key text, target_work text, target_generation bigint, target_head text,
    target_pack text, verdict_sequence bigint, report_pin text, assessment_data jsonb
) RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    draft craft_queue.publications%ROWTYPE;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    SELECT p.* INTO draft FROM craft_queue.publications AS p
    WHERE p.action_key = craft_queue.current_review_draft(target_work, target_generation, target_head, verdict_sequence)
      AND p.pack_digest = target_pack AND p.result_url = assessment_data->>'pull_request';
    IF NOT FOUND OR NOT craft_queue.github_readiness_lane(verifier_actor, draft.repository, draft.base_branch,
            assessment_data->>'policy_digest')
       OR NOT EXISTS (SELECT 1 FROM craft_queue.attempts AS a
            WHERE a.work_id = target_work AND a.generation = target_generation AND a.runner_id <> verifier_actor)
       OR draft.result_url <> 'https://github.com/' || draft.repository || '/pull/' || split_part(draft.result_url, '/pull/', 2)
       OR split_part(draft.result_url, '/pull/', 2) !~ '^[1-9][0-9]{0,9}$'
       OR EXISTS (SELECT 1 FROM craft_queue.shadow_merge_receipts AS latest
            WHERE latest.publication_key = draft.action_key AND latest.verification_sequence = verdict_sequence
              AND latest.policy_digest = assessment_data->>'policy_digest' AND latest.report_digest <> report_pin
              AND (latest.assessment->>'as_of')::bigint >= (assessment_data->>'as_of')::bigint)
       THEN RETURN NULL; END IF;
    RETURN craft_queue.record_shadow_merge(verifier_actor, receipt_key, target_work, target_generation, target_head,
        target_pack, verdict_sequence, report_pin, assessment_data);
END;
$$;

REVOKE ALL ON FUNCTION craft_queue.github_readiness_lane(text,text,text,text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.next_github_readiness(text,text,text,text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.record_github_readiness(text,text,text,bigint,text,text,bigint,text,jsonb) FROM PUBLIC;

COMMIT;
