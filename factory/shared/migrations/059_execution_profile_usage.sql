BEGIN;

CREATE FUNCTION craft_queue.execution_profile_usage(order_id text) RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    WITH selection AS (
        SELECT COALESCE(e.source_work_id, w.id) AS source_id, e.replacement_work_id AS replacement_id,
            w.repository, to_jsonb(e) AS escalation
        FROM craft_queue.work AS w
        LEFT JOIN craft_queue.profile_escalations AS e ON w.id IN (e.source_work_id, e.replacement_work_id)
        WHERE w.id = order_id AND jsonb_typeof(w.payload->'execution_profile') = 'object'
    ), usage AS MATERIALIZED (
        SELECT leg.position, w.id AS work_id, w.state, w.payload->'execution_profile' AS profile,
            CASE WHEN verdict.passed THEN 'passed' WHEN verdict.passed = false THEN 'failed' ELSE 'unscored' END
                AS independent_result,
            verdict.sequence AS verification_sequence, totals.*
        FROM selection AS plan
        CROSS JOIN LATERAL (VALUES (1, plan.source_id), (2, plan.replacement_id)) AS leg(position, id)
        JOIN craft_queue.work AS w ON w.id = leg.id
        LEFT JOIN craft_queue.attempts AS current_attempt
            ON current_attempt.work_id = w.id AND current_attempt.generation = w.generation
        LEFT JOIN LATERAL (
            SELECT v.sequence, v.passed FROM craft_queue.verifications AS v
            WHERE v.work_id = w.id AND v.generation = w.generation
                AND v.head = w.candidate AND v.digest = w.candidate_evidence
            ORDER BY v.sequence DESC LIMIT 1
        ) AS verdict ON w.state IN ('candidate', 'released') AND current_attempt.profile_started_at IS NOT NULL
        CROSS JOIN LATERAL (
            SELECT count(*) AS attempts, count(a.profile_started_at) AS supervised_starts,
                count(a.reported_tokens) AS attempts_with_usage,
                COALESCE(sum(a.reported_tokens), 0) AS reported_tokens,
                COALESCE(sum(a.token_limit) FILTER (WHERE a.reported_tokens IS NULL), 0)
                    AS reserved_tokens_without_usage
            FROM craft_queue.attempts AS a WHERE a.work_id = w.id
        ) AS totals
    )
    SELECT to_jsonb(selection) || jsonb_build_object(
        'orders', (SELECT jsonb_agg(to_jsonb(usage) - 'position' ORDER BY position) FROM usage),
        'totals', (SELECT jsonb_build_object(
            'attempts', sum(attempts), 'supervised_starts', sum(supervised_starts),
            'attempts_with_usage', sum(attempts_with_usage), 'reported_tokens', sum(reported_tokens),
            'reserved_tokens_without_usage', sum(reserved_tokens_without_usage)
        ) FROM usage)
    ) FROM selection;
$$;

REVOKE ALL ON FUNCTION craft_queue.execution_profile_usage(text) FROM PUBLIC;

COMMIT;
