BEGIN;

CREATE FUNCTION craft_queue.dispatch_status(order_id text) RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    WITH target AS MATERIALIZED (
        SELECT w.*, COALESCE(w.payload->>'agent_adapter', 'codex') AS adapter,
            (w.payload->'execution_profile'->>'max_attempts')::bigint AS attempt_cap,
            a.expires_at AS lease_expires_at,
            a.outcome = 'leased' AND a.expires_at > statement_timestamp() AS live_lease,
            w.repair_from_generation IS NOT NULL OR standing.work_id IS NOT NULL AS reserves_review_slot,
            craft_queue.series_blocker(w.id) AS predecessor
        FROM craft_queue.work AS w
        LEFT JOIN craft_queue.attempts AS a ON a.work_id = w.id AND a.generation = w.generation
        LEFT JOIN craft_queue.standing_work AS standing ON standing.work_id = w.id
        WHERE w.id = order_id
    ), enrollment AS (
        SELECT count(*) AS enrolled,
            count(*) FILTER (WHERE COALESCE(latest.mode, 'active') = 'active') AS active,
            count(*) FILTER (WHERE COALESCE(latest.mode, 'active') = 'active'
                AND craft_queue.standing_runner_allowed(target.id, enrolled.runner_id, statement_timestamp())) AS routed
        FROM target
        JOIN craft_queue.runner_repositories AS enrolled ON enrolled.repository = target.repository
        LEFT JOIN LATERAL (
            SELECT c.mode FROM craft_queue.runner_controls AS c WHERE c.runner_id = enrolled.runner_id
            ORDER BY c.revision DESC LIMIT 1
        ) AS latest ON true
    ), observed AS (
        SELECT target.*, enrollment.*, stats.summary,
            target.state IN ('queued', 'leased') AND NOT COALESCE(target.live_lease, false) AS waiting,
            craft_queue.standing_delegation_current(target.id, statement_timestamp()) AS standing_current
        FROM target CROSS JOIN enrollment
        CROSS JOIN LATERAL (SELECT craft_queue.operation_summary() AS summary) AS stats
    )
    SELECT jsonb_build_object(
        'work_id', id, 'repository', repository, 'state', state, 'generation', generation,
        'waiting_for_claim', waiting, 'live_lease', COALESCE(live_lease, false),
        'lease_expires_at', CASE WHEN state = 'leased' THEN lease_expires_at END,
        'required_adapter', adapter, 'requires_execution_profile', attempt_cap IS NOT NULL,
        'requires_project_context', EXISTS (SELECT 1 FROM craft_queue.project_context_pins(payload)),
        'database_enrolled_runners', enrolled, 'active_enrolled_runners', active,
        'active_runners_allowed_by_standing_rules', routed,
        'review_limit', summary->'review_limit', 'review_reserved', summary->'review_reserved',
        'uses_reserved_review_slot', reserves_review_slot,
        'attempt_token_limit', reserved_tokens,
        'queue_tokens_available', greatest((summary->>'token_budget')::numeric
            - (summary->>'tokens_charged_or_reserved')::numeric, 0),
        'waiting_on', predecessor,
        'blockers', CASE WHEN waiting THEN to_jsonb(array_remove(ARRAY[
            CASE WHEN (summary->>'paused')::boolean THEN 'queue_paused' END,
            CASE WHEN enrolled = 0 THEN 'no_enrolled_runner'
                 WHEN active = 0 THEN 'no_active_enrolled_runner'
                 WHEN standing_current AND routed = 0 THEN 'waiting_for_standing_runner' END,
            CASE WHEN NOT standing_current THEN 'standing_delegation_not_current' END,
            CASE WHEN predecessor IS NOT NULL THEN 'release_predecessor_not_ready' END,
            CASE WHEN generation >= attempt_cap THEN 'attempt_limit_reached' END,
            CASE WHEN adapter = 'claude' AND NOT EXISTS (
                SELECT 1 FROM craft_queue.claude_repository_approvals AS g
                WHERE g.repository = observed.repository AND g.revoked_at IS NULL
                  AND g.expires_at > statement_timestamp()
            ) THEN 'claude_repository_approval_required' END,
            CASE WHEN reserved_tokens > (summary->>'token_budget')::numeric
                - (summary->>'tokens_charged_or_reserved')::numeric THEN 'token_budget' END,
            CASE WHEN state = 'queued' AND NOT reserves_review_slot
                AND (summary->>'review_reserved')::bigint >= (summary->>'review_limit')::bigint
                THEN 'review_capacity_at_snapshot' END
        ]::text[], NULL)) ELSE '[]'::jsonb END
    ) FROM observed;
$$;

REVOKE ALL ON FUNCTION craft_queue.dispatch_status(text) FROM PUBLIC;

COMMIT;
