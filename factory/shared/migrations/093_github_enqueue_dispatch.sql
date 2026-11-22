BEGIN;

ALTER TABLE craft_queue.github_enqueue_intents
    ADD COLUMN scheduled boolean NOT NULL DEFAULT false;

CREATE FUNCTION craft_queue.claim_scheduled_github_enqueue(
    publisher_actor text, requested_policy_id text, local_policy_pin text
) RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    rule craft_queue.github_enqueue_policies%ROWTYPE;
    receipt craft_queue.shadow_merge_receipts%ROWTYPE;
    item craft_queue.github_enqueue_intents%ROWTYPE;
    claim_id text;
    now_at timestamptz;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    now_at := clock_timestamp();
    SELECT p.* INTO rule FROM craft_queue.github_enqueue_policies AS p
    WHERE p.id = requested_policy_id AND p.policy->>'publisher' = publisher_actor;
    IF NOT FOUND THEN RETURN NULL; END IF;

    -- Expiry is sufficient only when begin has never granted permission to send.
    UPDATE craft_queue.github_enqueue_intents AS i
    SET state = 'abandoned', resolution_actor = publisher_actor, resolution_at = now_at,
        resolution_evidence = 'Scheduled approval expired before send'
    WHERE i.standing_policy_id = requested_policy_id AND i.publisher = publisher_actor AND i.scheduled
        AND i.state = 'approved' AND NOT i.send_possible AND i.expires_at <= now_at;

    IF rule.revoked_at IS NOT NULL OR rule.expires_at <= now_at
        OR EXISTS (SELECT 1 FROM craft_queue.control WHERE paused)
        OR EXISTS (SELECT 1 FROM craft_queue.recoveries WHERE finished_at IS NULL)
        THEN RETURN '{}'::jsonb; END IF;

    SELECT i.* INTO item FROM craft_queue.github_enqueue_intents AS i
    JOIN craft_queue.shadow_merge_receipts AS r ON r.id = i.receipt_id
    WHERE i.standing_policy_id = requested_policy_id AND i.publisher = publisher_actor AND i.scheduled
        AND i.state = 'approved' AND i.expires_at > now_at
        AND r.assessment#>>'{github,policy_file_digest}' = local_policy_pin
        AND craft_queue.current_github_enqueue(r.id, r.head, r.report_digest) IS NOT NULL
        AND NOT EXISTS (SELECT 1 FROM craft_queue.shadow_merge_receipts AS newer
            WHERE newer.work_id = r.work_id AND newer.policy_digest = r.policy_digest
                AND (newer.recorded_at, newer.id) > (r.recorded_at, r.id)
                AND newer.assessment->'eligible' IS DISTINCT FROM 'true'::jsonb)
    ORDER BY i.approved_at, i.id LIMIT 1;
    IF FOUND THEN
        RETURN jsonb_build_object('intent', to_jsonb(item), 'candidate', craft_queue.candidate_pack(item.work_id));
    END IF;
    IF (SELECT count(*) FROM craft_queue.github_enqueue_intents AS i
        WHERE lower(i.repository) = lower(rule.repository) AND i.base_branch = rule.base_branch
            AND i.standing_policy_id IS NOT NULL AND (i.state <> 'abandoned' OR i.send_possible)
            AND i.approved_at > now_at - interval '24 hours') >= (rule.policy->>'daily_limit')::integer
        THEN RETURN '{}'::jsonb; END IF;

    FOR receipt IN
        WITH latest AS MATERIALIZED (
            SELECT DISTINCT ON (r.work_id) r.*
            FROM craft_queue.shadow_merge_receipts AS r
            WHERE r.repository = rule.repository AND r.policy_digest = rule.policy->>'shadow_policy_digest'
                AND r.recorded_at > now_at - interval '10 minutes'
            ORDER BY r.work_id, r.recorded_at DESC, r.id DESC
        )
        SELECT r.* FROM latest AS r
        JOIN craft_queue.publications AS p ON p.action_key = r.publication_key
        WHERE p.base_branch = rule.base_branch
            AND r.assessment#>>'{github,policy_file_digest}' = local_policy_pin
            AND r.assessment->'eligible' = 'true'::jsonb
            AND r.assessment->'native_verified' = 'true'::jsonb
            AND ((rule.policy->'allowed_paths') @> (r.assessment#>'{enqueue_risk,paths}'))
            AND jsonb_array_length(r.assessment#>'{enqueue_risk,paths}') <= (rule.policy->>'max_files')::integer
            AND (r.assessment#>>'{enqueue_risk,commits}')::integer <= (rule.policy->>'max_commits')::integer
            AND (r.assessment#>>'{enqueue_risk,changed_lines}')::integer <= (rule.policy->>'max_changed_lines')::integer
            AND NOT EXISTS (SELECT 1 FROM craft_queue.github_enqueue_intents AS i
                WHERE (i.standing_policy_id = requested_policy_id AND i.receipt_id = r.id AND i.scheduled)
                   OR (lower(i.repository) = lower(r.repository) AND i.pull_request = p.result_url
                       AND i.head = r.head AND (i.state <> 'abandoned' OR i.send_possible)))
            AND craft_queue.current_github_enqueue(r.id, r.head, r.report_digest) IS NOT NULL
        ORDER BY r.recorded_at, r.id
        LIMIT 1
    LOOP
        claim_id := 'scheduled-' || encode(sha256(convert_to(
            jsonb_build_array(requested_policy_id, receipt.id)::text, 'UTF8')), 'hex');
        IF craft_queue.authorize_github_enqueue_policy(publisher_actor, requested_policy_id, claim_id, receipt.id, receipt.head,
            receipt.report_digest)->>'state' = 'approved' THEN
            UPDATE craft_queue.github_enqueue_intents AS i SET scheduled = true WHERE i.id = claim_id
                RETURNING * INTO item;
            RETURN jsonb_build_object('intent', to_jsonb(item), 'candidate', craft_queue.candidate_pack(item.work_id));
        END IF;
    END LOOP;
    RETURN '{}'::jsonb;
END;
$$;

REVOKE ALL ON FUNCTION craft_queue.claim_scheduled_github_enqueue(text,text,text) FROM PUBLIC;

COMMIT;
