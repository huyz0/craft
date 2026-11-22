BEGIN;

ALTER FUNCTION craft_queue.current_github_enqueue(text,text,text)
    RENAME TO current_github_enqueue_before_delegation;

CREATE FUNCTION craft_queue.current_github_enqueue(receipt text, expected_head text, report_pin text)
RETURNS jsonb LANGUAGE sql STABLE SET search_path = pg_catalog AS $$
    SELECT craft_queue.current_github_enqueue_before_delegation(receipt, expected_head, report_pin)
    FROM craft_queue.shadow_merge_receipts AS r
    JOIN craft_queue.work AS w ON w.id = r.work_id
    WHERE r.id = receipt
      AND craft_queue.standing_delegation_current(w.id, statement_timestamp())
      AND craft_queue.team_skill_allowed(w.repository, w.payload)
      AND craft_queue.project_context_allowed(w.repository, w.payload);
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
        RETURNING w.id
    )
    UPDATE craft_queue.attempts AS a SET outcome = 'cancelled'
    FROM cancelled WHERE a.work_id = cancelled.id AND a.outcome = 'leased';
$$;

REVOKE ALL ON FUNCTION craft_queue.current_github_enqueue(text,text,text) FROM PUBLIC;

COMMIT;
