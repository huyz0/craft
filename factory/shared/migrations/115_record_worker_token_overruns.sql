BEGIN;

ALTER TABLE craft_queue.attempts
    DROP CONSTRAINT attempts_check,
    ADD CONSTRAINT attempts_reported_tokens_check CHECK (
        reported_tokens >= 0 AND (reported_tokens <= token_limit OR
            (worker_stop_reported_at IS NOT NULL AND outcome <> 'candidate'))
    );

CREATE OR REPLACE FUNCTION craft_queue.finish_failed_attempt(
    runner text, order_id text, lease_generation bigint, tokens bigint
) RETURNS boolean
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    recorded boolean;
    now_at timestamptz;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    IF tokens < 0 THEN RETURN false; END IF;
    now_at := clock_timestamp();
    UPDATE craft_queue.attempts AS a SET expires_at = now_at, worker_stop_reported_at = now_at,
        reported_tokens = tokens
    FROM craft_queue.work AS w
    WHERE w.id = order_id AND a.work_id = w.id AND a.generation = lease_generation
      AND craft_queue.runner_context_allowed(w, a, runner, lease_generation)
      AND a.expires_at > now_at AND a.profile_started_at IS NOT NULL
      AND a.worker_stop_reported_at IS NULL AND a.reported_tokens IS NULL
    RETURNING true INTO recorded;
    IF recorded THEN RETURN true; END IF;

    RETURN EXISTS (
        SELECT 1 FROM craft_queue.attempts AS a
        JOIN craft_queue.work AS w ON w.id = a.work_id
        JOIN craft_queue.runner_repositories AS enrollment
          ON enrollment.repository = w.repository AND enrollment.runner_id = runner
        WHERE a.work_id = order_id AND a.generation = lease_generation AND a.runner_id = runner
          AND a.worker_stop_reported_at IS NOT NULL
          AND a.reported_tokens IS NOT DISTINCT FROM tokens
    );
END;
$$;

CREATE OR REPLACE FUNCTION craft_queue.submit_candidate(
    runner text, lease_work_id text, lease_generation bigint,
    commit_id text, evidence_ref text, tokens bigint
) RETURNS boolean
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    now_at timestamptz;
    selected craft_queue.work%ROWTYPE;
    attempt craft_queue.attempts%ROWTYPE;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    now_at := clock_timestamp();
    IF COALESCE(commit_id, '') !~ '^([a-f0-9]{40}|[a-f0-9]{64})$'
        OR length(COALESCE(evidence_ref, '')) NOT BETWEEN 1 AND 500 OR tokens < 0 THEN
        RAISE EXCEPTION 'invalid candidate, evidence reference, or usage';
    END IF;
    SELECT w.* INTO selected FROM craft_queue.work AS w
    JOIN craft_queue.runner_repositories AS grant_row ON grant_row.repository = w.repository
    WHERE w.id = lease_work_id AND w.generation = lease_generation AND grant_row.runner_id = runner;
    IF NOT FOUND THEN RETURN false; END IF;
    SELECT * INTO attempt FROM craft_queue.attempts AS a
    WHERE a.work_id = selected.id AND a.generation = selected.generation AND a.runner_id = runner;
    IF NOT FOUND THEN RETURN false; END IF;
    IF selected.state = 'candidate' AND attempt.outcome = 'candidate' THEN
        RETURN selected.candidate = commit_id AND selected.candidate_evidence = evidence_ref
             AND attempt.reported_tokens IS NOT DISTINCT FROM tokens;
    END IF;
    IF selected.state <> 'leased' OR attempt.outcome <> 'leased' OR attempt.expires_at <= now_at
       OR NOT craft_queue.standing_runner_allowed(selected.id, runner, now_at) THEN
        RETURN false;
    END IF;
    IF tokens > attempt.token_limit THEN
        PERFORM craft_queue.finish_failed_attempt(runner, selected.id, selected.generation, tokens);
        RETURN false;
    END IF;
    UPDATE craft_queue.attempts AS a SET outcome = 'candidate', reported_tokens = tokens
    WHERE a.work_id = selected.id AND a.generation = selected.generation;
    UPDATE craft_queue.work AS w SET state = 'candidate', candidate = commit_id, candidate_evidence = evidence_ref
    WHERE w.id = selected.id;
    RETURN true;
END;
$$;

COMMIT;
