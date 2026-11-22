BEGIN;

CREATE OR REPLACE FUNCTION craft_queue.queue_usage(repo text, window_hours integer, maximum integer)
RETURNS jsonb LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    WITH bounds AS (
        SELECT statement_timestamp() AS as_of,
               statement_timestamp() - make_interval(hours => window_hours) AS since
        WHERE window_hours BETWEEN 1 AND 720 AND maximum BETWEEN 1 AND 100
    ), observed AS NOT MATERIALIZED (
        SELECT w.repository, COALESCE(w.payload->>'agent_adapter', 'codex') AS adapter,
               w.payload#>>'{execution_profile,model}' AS model,
               w.payload#>>'{execution_profile,reasoning_effort}' AS reasoning_effort,
               a.work_id, a.generation, a.runner_id, a.started_at, a.profile_started_at, a.outcome,
               a.token_limit, a.reported_tokens
        FROM bounds
        JOIN craft_queue.attempts AS a ON a.started_at >= bounds.since AND a.started_at <= bounds.as_of
        JOIN craft_queue.work AS w ON w.id = a.work_id
        WHERE repo IS NULL OR w.repository = repo
    ), grouped AS MATERIALIZED (
        SELECT repository, adapter, model, reasoning_effort,
               count(*) AS attempts, count(profile_started_at) AS supervised_starts,
               count(*) FILTER (WHERE outcome = 'candidate') AS candidate_attempts,
               count(reported_tokens) AS attempts_with_usage,
               COALESCE(sum(reported_tokens), 0) AS reported_tokens,
               COALESCE(sum(token_limit) FILTER (WHERE reported_tokens IS NULL), 0)
                   AS reserved_tokens_without_usage,
               count(*) FILTER (WHERE reported_tokens > token_limit) AS overrun_attempts,
               COALESCE(sum(reported_tokens - token_limit) FILTER (WHERE reported_tokens > token_limit), 0)
                   AS tokens_above_reservation
        FROM observed
        GROUP BY 1, 2, 3, 4
    ), overruns AS (
        SELECT repository, work_id, generation, runner_id, started_at, adapter, model, reasoning_effort,
               token_limit, reported_tokens, reported_tokens - token_limit AS tokens_above_reservation
        FROM observed
        WHERE reported_tokens > token_limit
        ORDER BY tokens_above_reservation DESC, started_at DESC, work_id, generation
        LIMIT maximum
    ), ranked AS (
        SELECT *, row_number() OVER (
            ORDER BY attempts DESC, repository, adapter, model NULLS LAST, reasoning_effort NULLS LAST
        ) AS position FROM grouped
    )
    SELECT jsonb_build_object(
        'since', bounds.since, 'as_of', bounds.as_of, 'repository', repo,
        'total_groups', (SELECT count(*) FROM grouped),
        'totals', (SELECT jsonb_build_object(
            'attempts', COALESCE(sum(attempts), 0),
            'supervised_starts', COALESCE(sum(supervised_starts), 0),
            'candidate_attempts', COALESCE(sum(candidate_attempts), 0),
            'attempts_with_usage', COALESCE(sum(attempts_with_usage), 0),
            'reported_tokens', COALESCE(sum(reported_tokens), 0),
            'overrun_attempts', COALESCE(sum(overrun_attempts), 0),
            'tokens_above_reservation', COALESCE(sum(tokens_above_reservation), 0),
            'reserved_tokens_without_usage', COALESCE(sum(reserved_tokens_without_usage), 0)
        ) FROM grouped),
        'groups', COALESCE((SELECT jsonb_agg(to_jsonb(ranked) - 'position' ORDER BY position)
                            FROM ranked WHERE position <= maximum), '[]'::jsonb),
        'overruns', COALESCE((SELECT jsonb_agg(to_jsonb(overruns)
            ORDER BY tokens_above_reservation DESC, started_at DESC, work_id, generation) FROM overruns), '[]'::jsonb)
    )
    FROM bounds;
$$;

REVOKE ALL ON FUNCTION craft_queue.queue_usage(text, integer, integer) FROM PUBLIC;

COMMIT;
