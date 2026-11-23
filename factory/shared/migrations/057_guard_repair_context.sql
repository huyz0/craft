BEGIN;

CREATE OR REPLACE FUNCTION craft_queue.repair_context(runner text, order_id text, current_generation bigint)
RETURNS jsonb
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT jsonb_build_object(
        'head', r.head, 'native_url', r.native_url, 'note', r.note, 'evidence', r.evidence,
        'digest', p.digest, 'pack_base64', replace(encode(p.payload, 'base64'), E'\n', '')
    )
    FROM craft_queue.work AS w JOIN craft_queue.repair_requests AS r
      ON r.work_id = w.id AND r.from_generation = w.repair_from_generation
    JOIN craft_queue.candidate_packs AS p ON p.work_id = r.work_id AND p.generation = r.from_generation
    JOIN craft_queue.attempts AS a ON a.work_id = w.id AND a.generation = w.generation
    JOIN craft_queue.runner_repositories AS allowed ON allowed.repository = w.repository
    WHERE w.id = order_id AND w.state = 'leased' AND w.generation = current_generation
      AND a.runner_id = runner AND allowed.runner_id = runner
      AND a.outcome = 'leased' AND a.expires_at > statement_timestamp()
      AND (w.payload->>'agent_adapter' IS DISTINCT FROM 'claude' OR a.claude_approval_id IS NOT NULL)
      AND craft_queue.team_skill_allowed(w.repository, w.payload)
      AND craft_queue.project_context_allowed(w.repository, w.payload)
      AND craft_queue.series_blocker(w.id) IS NULL
      AND craft_queue.standing_runner_allowed(w.id, runner, statement_timestamp());
$$;

COMMIT;
