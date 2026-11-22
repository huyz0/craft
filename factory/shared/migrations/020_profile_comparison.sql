BEGIN;

CREATE FUNCTION craft_queue.compare_execution_profiles(pairs jsonb) RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    WITH matched AS MATERIALIZED (
        SELECT p.case_id, b.id AS baseline, c.id AS challenger,
            b.payload->'execution_profile' AS baseline_profile,
            c.payload->'execution_profile' AS challenger_profile
        FROM jsonb_to_recordset(pairs) AS p(case_id text, baseline text, challenger text)
        JOIN craft_queue.work AS b ON b.id = p.baseline
        JOIN craft_queue.work AS c ON c.id = p.challenger
        WHERE b.repository = c.repository AND b.reserved_tokens = c.reserved_tokens
            AND b.payload - ARRAY['id', 'repository', 'execution_profile']
                = c.payload - ARRAY['id', 'repository', 'execution_profile']
            AND ROW(b.payload#>'{execution_profile,model}', b.payload#>'{execution_profile,reasoning_effort}',
                    b.payload#>'{execution_profile,timeout_seconds}', b.payload#>'{execution_profile,max_attempts}')
                <> ROW(c.payload#>'{execution_profile,model}', c.payload#>'{execution_profile,reasoning_effort}',
                       c.payload#>'{execution_profile,timeout_seconds}', c.payload#>'{execution_profile,max_attempts}')
    ), validated AS (
        SELECT jsonb_agg(DISTINCT baseline_profile)->0 AS baseline_profile,
            jsonb_agg(DISTINCT challenger_profile)->0 AS challenger_profile
        FROM matched
        HAVING count(*) = jsonb_array_length(pairs)
            AND count(DISTINCT baseline_profile) = 1 AND count(DISTINCT challenger_profile) = 1
    ), results AS MATERIALIZED (
        SELECT p.case_id, arm.side, w.id AS work_id, w.repository, w.state,
            w.generation, w.candidate AS head, v.sequence AS verification_sequence,
            v.evidence AS verification_evidence,
            CASE
                WHEN w.state IN ('candidate', 'released') AND v.passed THEN 'passed'
                WHEN w.state IN ('candidate', 'released') AND v.passed = false THEN 'failed'
                ELSE 'unscored'
            END AS independent_result,
            usage.attempts, usage.supervised_starts, usage.attempts_with_usage, usage.reported_tokens,
            usage.reserved_tokens_without_usage
        FROM matched AS p
        CROSS JOIN LATERAL (VALUES ('baseline', p.baseline), ('challenger', p.challenger)) AS arm(side, id)
        JOIN craft_queue.work AS w ON w.id = arm.id
        LEFT JOIN craft_queue.attempts AS current_attempt
            ON current_attempt.work_id = w.id AND current_attempt.generation = w.generation
        LEFT JOIN LATERAL (
            SELECT sequence, evidence, passed FROM craft_queue.verifications AS v
            WHERE v.work_id = w.id AND v.generation = w.generation
                AND v.head = w.candidate AND v.digest = w.candidate_evidence
            ORDER BY sequence DESC LIMIT 1
        ) AS v ON w.state IN ('candidate', 'released') AND current_attempt.profile_started_at IS NOT NULL
        CROSS JOIN LATERAL (
            SELECT count(*) AS attempts, count(a.profile_started_at) AS supervised_starts,
                count(a.reported_tokens) AS attempts_with_usage,
                COALESCE(sum(a.reported_tokens), 0) AS reported_tokens,
                COALESCE(sum(a.token_limit) FILTER (WHERE a.reported_tokens IS NULL), 0)
                    AS reserved_tokens_without_usage
            FROM craft_queue.attempts AS a WHERE a.work_id = w.id
        ) AS usage
    ), totals AS (
        SELECT side, count(*) AS cases,
            count(*) FILTER (WHERE independent_result = 'passed') AS passed,
            count(*) FILTER (WHERE independent_result = 'failed') AS failed,
            count(*) FILTER (WHERE independent_result = 'unscored') AS unscored,
            sum(attempts) AS attempts, sum(supervised_starts) AS supervised_starts,
            sum(attempts_with_usage) AS attempts_with_usage,
            sum(reported_tokens) AS reported_tokens,
            sum(reserved_tokens_without_usage) AS reserved_tokens_without_usage
        FROM results GROUP BY side
    )
    SELECT jsonb_build_object(
        'baseline_profile', validated.baseline_profile, 'challenger_profile', validated.challenger_profile,
        'totals', (SELECT jsonb_object_agg(side, to_jsonb(totals) - 'side') FROM totals),
        'results', (SELECT jsonb_agg(to_jsonb(results) ORDER BY case_id, side) FROM results)
    )
    FROM validated;
$$;

REVOKE ALL ON FUNCTION craft_queue.compare_execution_profiles(jsonb) FROM PUBLIC;
COMMIT;
