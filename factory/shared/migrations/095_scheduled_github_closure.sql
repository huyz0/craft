BEGIN;

ALTER TABLE craft_queue.github_work_closures
    ADD COLUMN enqueue_intent_id text REFERENCES craft_queue.github_enqueue_intents(id),
    ADD COLUMN standing_policy_id text REFERENCES craft_queue.github_enqueue_policies(id);

CREATE INDEX github_enqueue_pending_closure ON craft_queue.github_enqueue_intents(repository, base_branch, id)
    WHERE scheduled AND state = 'merged' AND queue_entry_id IS NOT NULL;

CREATE FUNCTION craft_queue.close_scheduled_github_work(
    verifier_actor text, lane_repository text, lane_branch text, policy_file_pin text, requested_limit integer
) RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    target record;
    closure jsonb;
    closed jsonb := '[]'::jsonb;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    IF EXISTS (SELECT 1 FROM craft_queue.control WHERE paused)
        OR EXISTS (SELECT 1 FROM craft_queue.recoveries WHERE finished_at IS NULL)
        THEN RETURN closed; END IF;

    FOR target IN
        SELECT i.id, i.work_id, i.head, i.standing_policy_id, i.merge_observation_receipt_id
        FROM craft_queue.github_enqueue_intents AS i
        JOIN craft_queue.github_enqueue_policies AS rule ON rule.id = i.standing_policy_id
        JOIN craft_queue.shadow_merge_receipts AS ready ON ready.id = i.receipt_id
        JOIN craft_queue.github_shadow_merges AS m ON m.receipt_id = i.merge_observation_receipt_id
        JOIN craft_queue.attempts AS a ON a.work_id = ready.work_id AND a.generation = ready.generation
        JOIN LATERAL (
            SELECT v.sequence, v.passed, v.head, v.digest FROM craft_queue.verifications AS v
            WHERE v.work_id = ready.work_id AND v.generation = ready.generation
            ORDER BY v.sequence DESC LIMIT 1
        ) AS latest ON latest.passed AND latest.head = ready.head AND latest.digest = ready.pack_digest
        WHERE i.repository = lane_repository AND i.base_branch = lane_branch AND i.scheduled
            AND i.state = 'merged' AND i.queue_entry_id IS NOT NULL
            AND rule.policy->'close_standalone_work' = 'true'::jsonb
            AND rule.revoked_at IS NULL AND rule.expires_at > statement_timestamp()
            AND ready.assessment#>>'{github,policy_file_digest}' = policy_file_pin
            AND rule.policy->>'shadow_policy_digest' = ready.policy_digest
            AND verifier_actor NOT IN (i.publisher, ready.actor, a.runner_id)
            AND m.verifier NOT IN (i.publisher, ready.actor, a.runner_id)
            AND ready.publication_key = craft_queue.current_review_draft(
                ready.work_id, ready.generation, ready.head, latest.sequence)
            AND NOT craft_queue.github_enqueue_blocks_work(i.work_id)
            AND NOT EXISTS (SELECT 1 FROM craft_queue.series_members AS s WHERE s.work_id = i.work_id)
            AND NOT EXISTS (SELECT 1 FROM craft_queue.github_work_closures AS c WHERE c.work_id = i.work_id)
            AND NOT EXISTS (SELECT 1 FROM craft_queue.publications AS p
                WHERE p.work_id = i.work_id AND p.state IN ('reserved', 'sending', 'uncertain'))
        ORDER BY m.recorded_at, i.id
        LIMIT least(greatest(requested_limit, 1), 20)
    LOOP
        closure := craft_queue.close_github_work(verifier_actor, target.work_id,
            target.merge_observation_receipt_id, target.head,
            'Scheduled GitHub enqueue ' || target.id || ' under policy ' || target.standing_policy_id);
        IF closure IS NOT NULL THEN
            UPDATE craft_queue.github_work_closures AS c
            SET enqueue_intent_id = target.id, standing_policy_id = target.standing_policy_id
            WHERE c.work_id = target.work_id RETURNING to_jsonb(c) INTO closure;
            closed := closed || jsonb_build_array(closure);
        END IF;
    END LOOP;
    RETURN closed;
END;
$$;

REVOKE ALL ON FUNCTION craft_queue.close_scheduled_github_work(text,text,text,text,integer) FROM PUBLIC;

COMMIT;
