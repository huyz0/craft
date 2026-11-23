BEGIN;

ALTER FUNCTION craft_queue.start_profiled_attempt(text, text, bigint) RENAME TO start_attempt;

CREATE OR REPLACE FUNCTION craft_queue.start_attempt(runner text, lease_work_id text, lease_generation bigint)
RETURNS timestamptz
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    now_at timestamptz;
    started timestamptz;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    now_at := clock_timestamp();
    UPDATE craft_queue.attempts AS a SET profile_started_at = now_at
    FROM craft_queue.work AS w, craft_queue.runner_repositories AS grant_row
    WHERE w.id = lease_work_id AND w.generation = lease_generation AND w.state = 'leased'
      AND a.work_id = w.id AND a.generation = w.generation AND a.runner_id = runner
      AND a.outcome = 'leased' AND a.expires_at > now_at AND a.profile_started_at IS NULL
      AND grant_row.runner_id = runner AND grant_row.repository = w.repository
      AND (w.payload->>'agent_adapter' IS DISTINCT FROM 'claude' OR a.claude_approval_id IS NOT NULL)
      AND craft_queue.series_blocker(w.id) IS NULL
      AND craft_queue.standing_runner_allowed(w.id, runner, now_at)
    RETURNING a.profile_started_at INTO started;
    RETURN started;
END;
$$;

COMMIT;
