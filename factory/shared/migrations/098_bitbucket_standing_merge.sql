BEGIN;

CREATE FUNCTION craft_queue.current_bitbucket_merge(order_id text, generation_pin bigint, head_pin text, pack_pin text)
RETURNS boolean LANGUAGE sql STABLE SET search_path = pg_catalog AS $$
    SELECT EXISTS (
        SELECT 1 FROM craft_queue.publications AS p
        JOIN craft_queue.bitbucket_work_merges AS m ON m.publication_key = p.action_key
        WHERE p.work_id = order_id AND p.state = 'confirmed' AND p.generation = generation_pin
          AND p.head = head_pin AND p.pack_digest = pack_pin AND m.observation->>'head' = head_pin
    );
$$;

CREATE OR REPLACE FUNCTION craft_queue.fence_standing_work(policy_id text, only_issue text, reason text)
RETURNS void
LANGUAGE sql SET search_path = pg_catalog AS $$
    WITH blocked AS (
        UPDATE craft_queue.standing_work AS binding SET blocked_at = clock_timestamp()
        WHERE binding.rule_id = policy_id AND (only_issue IS NULL OR binding.issue_key = only_issue)
          AND binding.blocked_at IS NULL
        RETURNING binding.work_id
    ), cancelled AS (
        UPDATE craft_queue.work AS w SET state = 'cancelled', cancellation_reason = reason
        FROM blocked WHERE w.id = blocked.work_id AND w.state IN ('queued', 'leased', 'candidate')
          AND NOT craft_queue.github_enqueue_blocks_work(w.id)
          AND NOT EXISTS (
              SELECT 1 FROM craft_queue.github_enqueue_intents AS i
              WHERE i.work_id = w.id AND i.head = w.candidate AND i.state = 'merged'
                AND i.merge_observation_receipt_id IS NOT NULL
          )
          AND NOT EXISTS (
              SELECT 1 FROM craft_queue.publications AS p
              WHERE p.work_id = w.id AND p.state IN ('reserved', 'sending', 'uncertain')
          )
          AND NOT craft_queue.current_bitbucket_merge(w.id, w.generation, w.candidate, w.candidate_evidence)
        RETURNING w.id
    )
    UPDATE craft_queue.attempts AS a SET outcome = 'cancelled'
    FROM cancelled WHERE a.work_id = cancelled.id AND a.outcome = 'leased';
$$;

ALTER FUNCTION craft_queue.request_repair(text,text,bigint,text,text,text,text)
    RENAME TO request_repair_before_bitbucket_merge;
CREATE FUNCTION craft_queue.request_repair(
    actor text, order_id text, candidate_generation bigint, candidate_head text,
    draft_url text, evidence_ref text, feedback text
) RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    IF EXISTS (
        SELECT 1 FROM craft_queue.work AS w WHERE w.id = order_id AND w.state = 'candidate'
          AND w.generation = candidate_generation AND w.candidate = candidate_head
          AND craft_queue.current_bitbucket_merge(w.id, w.generation, w.candidate, w.candidate_evidence)
    ) THEN RETURN NULL; END IF;
    RETURN craft_queue.request_repair_before_bitbucket_merge(
        actor, order_id, candidate_generation, candidate_head, draft_url, evidence_ref, feedback);
END;
$$;

ALTER FUNCTION craft_queue.cancel(text,text) RENAME TO cancel_before_bitbucket_merge;
CREATE FUNCTION craft_queue.cancel(work_id text, reason text) RETURNS boolean
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    IF EXISTS (
        SELECT 1 FROM craft_queue.work AS w WHERE w.id = $1 AND w.state = 'candidate'
          AND craft_queue.current_bitbucket_merge(w.id, w.generation, w.candidate, w.candidate_evidence)
    )
    THEN RAISE EXCEPTION 'close the confirmed Bitbucket merge before changing the work state'; END IF;
    RETURN craft_queue.cancel_before_bitbucket_merge(work_id, reason);
END;
$$;

REVOKE ALL ON FUNCTION craft_queue.current_bitbucket_merge(text,bigint,text,text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.request_repair(text,text,bigint,text,text,text,text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.cancel(text,text) FROM PUBLIC;

COMMIT;
