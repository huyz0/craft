BEGIN;

CREATE FUNCTION craft_queue.report_failed_attempt_usage(
    runner text, order_id text, lease_generation bigint, tokens bigint
) RETURNS boolean
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    reported boolean;
    now_at timestamptz;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    now_at := clock_timestamp();
    UPDATE craft_queue.attempts AS a SET reported_tokens = tokens, expires_at = now_at
    FROM craft_queue.work AS w
    WHERE w.id = order_id AND a.work_id = w.id AND a.generation = lease_generation
      AND craft_queue.runner_context_allowed(w, a, runner, lease_generation)
      AND a.expires_at > now_at AND a.reported_tokens IS NULL AND tokens BETWEEN 0 AND a.token_limit
      AND (w.payload->>'execution_profile' IS NULL OR a.profile_started_at IS NOT NULL)
    RETURNING true INTO reported;
    IF reported THEN RETURN true; END IF;

    RETURN EXISTS (
        SELECT 1 FROM craft_queue.attempts AS a
        JOIN craft_queue.work AS w ON w.id = a.work_id
        JOIN craft_queue.runner_repositories AS enrollment
          ON enrollment.repository = w.repository AND enrollment.runner_id = runner
        WHERE a.work_id = order_id AND a.generation = lease_generation AND a.runner_id = runner
          AND a.outcome <> 'candidate' AND a.expires_at <= now_at AND a.reported_tokens = tokens
    );
END;
$$;

REVOKE ALL ON FUNCTION craft_queue.report_failed_attempt_usage(text,text,bigint,bigint) FROM PUBLIC;

COMMIT;
