BEGIN;

CREATE FUNCTION craft_queue.github_readiness_backlog(repo_filter text, item_limit integer)
RETURNS jsonb LANGUAGE sql STABLE SET search_path = pg_catalog AS $$
    WITH inspection AS (
        SELECT w.id AS work_id, w.repository, w.candidate AS head, v.sequence AS verification_sequence,
            draft.result_url AS pull_request, rule.policy->>'shadow_policy_digest' AS policy_digest,
            attempt.attempted_at, receipt.id AS receipt_id, receipt.recorded_at AS checked_at,
            receipt.assessment->'blockers' AS blockers,
            CASE
                WHEN attempt.attempted_at IS NOT NULL
                  AND (receipt.recorded_at IS NULL OR attempt.attempted_at > receipt.recorded_at)
                    THEN CASE WHEN attempt.attempted_at <= statement_timestamp() - interval '90 seconds'
                              THEN 'check_overdue' ELSE 'check_in_progress' END
                WHEN receipt.assessment->'eligible' = 'false'::jsonb THEN 'blocked'
                WHEN receipt.recorded_at > statement_timestamp() - interval '10 minutes' THEN 'checked'
                ELSE 'waiting_for_check'
            END AS state
        FROM craft_queue.github_enqueue_policies AS rule
        JOIN craft_queue.shadow_policies AS pin ON pin.repository = rule.repository
            AND pin.digest = rule.policy->>'shadow_policy_digest' AND pin.require_native
            AND pin.expires_at > statement_timestamp()
        JOIN craft_queue.work AS w ON w.repository = rule.repository AND w.state = 'candidate'
        JOIN LATERAL (
            SELECT latest.sequence FROM craft_queue.verifications AS latest
            WHERE latest.work_id = w.id AND latest.generation = w.generation
            ORDER BY latest.sequence DESC LIMIT 1
        ) AS v ON true
        JOIN craft_queue.publications AS draft ON draft.action_key =
            craft_queue.current_review_draft(w.id, w.generation, w.candidate, v.sequence)
        LEFT JOIN craft_queue.github_readiness_attempts AS attempt ON attempt.publication_key = draft.action_key
            AND attempt.verification_sequence = v.sequence AND attempt.policy_digest = pin.digest
        LEFT JOIN LATERAL (
            SELECT r.id, r.assessment, r.recorded_at FROM craft_queue.shadow_merge_receipts AS r
            WHERE r.publication_key = draft.action_key AND r.verification_sequence = v.sequence
              AND r.policy_digest = pin.digest
            ORDER BY r.recorded_at DESC, r.id DESC LIMIT 1
        ) AS receipt ON true
        WHERE rule.revoked_at IS NULL AND rule.expires_at > statement_timestamp()
          AND (repo_filter IS NULL OR w.repository = repo_filter)
          AND draft.base_branch = rule.base_branch
          AND draft.result_url = 'https://github.com/' || w.repository || '/pull/' || split_part(draft.result_url, '/pull/', 2)
          AND split_part(draft.result_url, '/pull/', 2) ~ '^[1-9][0-9]{0,9}$'
          AND NOT EXISTS (SELECT 1 FROM craft_queue.github_enqueue_intents AS i
              WHERE i.repository = w.repository AND i.pull_request = draft.result_url AND i.head = w.candidate
                AND (i.state <> 'abandoned' OR i.send_possible))
    ), pending AS MATERIALIZED (
        SELECT *, CASE state WHEN 'check_overdue' THEN 1 WHEN 'blocked' THEN 2
                              WHEN 'waiting_for_check' THEN 3 ELSE 4 END AS priority
        FROM inspection WHERE state <> 'checked'
    ), page AS (
        SELECT * FROM pending ORDER BY priority, greatest(attempted_at, checked_at) NULLS FIRST, work_id COLLATE "C"
        LIMIT item_limit
    )
    SELECT jsonb_build_object(
        'total', (SELECT count(*) FROM pending),
        'items', COALESCE((
            SELECT jsonb_agg(jsonb_strip_nulls(to_jsonb(page) - 'priority' - 'blockers') ||
                jsonb_build_object('blockers', COALESCE((
                    SELECT jsonb_agg(code ORDER BY position)
                    FROM jsonb_array_elements(page.blockers) WITH ORDINALITY AS reason(code, position)
                    WHERE position <= 3
                ), '[]'::jsonb))
                ORDER BY priority, greatest(attempted_at, checked_at) NULLS FIRST, work_id COLLATE "C")
            FROM page
        ), '[]'::jsonb)
    );
$$;

ALTER FUNCTION craft_queue.operator_brief(text,integer,integer) RENAME TO operator_brief_before_readiness;

CREATE FUNCTION craft_queue.operator_brief(repo_filter text, usage_hours integer, item_limit integer)
RETURNS jsonb LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT craft_queue.operator_brief_before_readiness(repo_filter, usage_hours, item_limit)
        || jsonb_build_object('github_readiness', craft_queue.github_readiness_backlog(repo_filter, item_limit));
$$;

REVOKE ALL ON FUNCTION craft_queue.github_readiness_backlog(text,integer) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.operator_brief(text,integer,integer) FROM PUBLIC;

COMMIT;
