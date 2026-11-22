BEGIN;

CREATE FUNCTION craft_queue.operator_brief(repo_filter text, usage_hours integer, item_limit integer)
RETURNS jsonb LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    WITH inbox AS MATERIALIZED (
        SELECT craft_queue.review_inbox(repo_filter, NULL, item_limit, NULL) AS snapshot
    ), backlog AS MATERIALIZED (
        SELECT w.id AS work_id, w.repository, 'queued' AS kind, w.created_at AS since, NULL::text AS runner_id
        FROM craft_queue.work AS w
        WHERE w.state = 'queued' AND (repo_filter IS NULL OR w.repository = repo_filter)
        UNION ALL
        SELECT w.id, w.repository, 'expired_lease', a.expires_at, a.runner_id
        FROM craft_queue.work AS w
        JOIN craft_queue.attempts AS a ON a.work_id = w.id AND a.generation = w.generation
        WHERE w.state = 'leased' AND a.outcome = 'leased' AND a.expires_at <= statement_timestamp()
          AND (repo_filter IS NULL OR w.repository = repo_filter)
    ), backlog_page AS (
        SELECT *, row_number() OVER (PARTITION BY kind ORDER BY since, work_id COLLATE "C") AS position
        FROM backlog
    ), enqueue AS MATERIALIZED (
        SELECT i.id AS intent_id, i.work_id, i.repository, i.pull_request, i.head, i.state,
               i.sending_at
        FROM craft_queue.github_enqueue_intents AS i
        WHERE i.state IN ('sending', 'uncertain', 'queued')
          AND (repo_filter IS NULL OR i.repository = repo_filter)
    ), enqueue_page AS (
        SELECT * FROM enqueue ORDER BY (state = 'queued'), sending_at, intent_id COLLATE "C" LIMIT item_limit
    )
    SELECT jsonb_build_object(
        'as_of', statement_timestamp(), 'repository', repo_filter,
        'queue', craft_queue.operation_summary() || craft_queue.review_wait_summary(),
        'review', jsonb_build_object('total', snapshot->'total', 'counts', snapshot->'counts',
            'items', COALESCE((SELECT jsonb_agg(jsonb_strip_nulls(jsonb_build_object(
                'work_id', item->'work_id', 'repository', item->'repository', 'issue', item->'issue',
                'next_action', item->'next_action', 'head', item->'head',
                'draft_url', item#>'{publication,result_url}',
                'publication_key', item#>'{publication,action_key}',
                'verification_evidence', item#>'{independent_verification,evidence}',
                'review_handoff_id', item#>'{pending_repair,handoff_id}',
                'review_evidence', item#>'{pending_repair,evidence}',
                'merge_receipt_id', item#>'{merge_closure,merge_receipt_id}'
            )) ORDER BY position) FROM jsonb_array_elements(snapshot->'items')
                WITH ORDINALITY AS entries(item, position)), '[]'::jsonb)),
        'assignments', craft_queue.review_wait_backlog(repo_filter, item_limit),
        'backlog', jsonb_build_object(
            'queued_total', (SELECT count(*) FROM backlog WHERE kind = 'queued'),
            'expired_lease_total', (SELECT count(*) FROM backlog WHERE kind = 'expired_lease'),
            'items', COALESCE((SELECT jsonb_agg(to_jsonb(backlog_page) - 'position' ORDER BY kind, position)
                FROM backlog_page WHERE position <= item_limit), '[]'::jsonb)),
        'github_queue', jsonb_build_object(
            'unresolved_total', (SELECT count(*) FROM enqueue WHERE state <> 'queued'),
            'queued_total', (SELECT count(*) FROM enqueue WHERE state = 'queued'),
            'items', COALESCE((SELECT jsonb_agg(to_jsonb(enqueue_page)
                ORDER BY (state = 'queued'), sending_at, intent_id COLLATE "C") FROM enqueue_page), '[]'::jsonb)),
        'usage', craft_queue.queue_usage(repo_filter, usage_hours, item_limit))
    FROM inbox;
$$;

REVOKE ALL ON FUNCTION craft_queue.operator_brief(text,integer,integer) FROM PUBLIC;

COMMIT;
