BEGIN;

ALTER TABLE craft_queue.attempts ADD COLUMN worker_stop_reported_at timestamptz;

CREATE FUNCTION craft_queue.finish_failed_attempt(
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
        reported_tokens = CASE WHEN tokens BETWEEN 0 AND a.token_limit THEN tokens END
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
          AND a.reported_tokens IS NOT DISTINCT FROM CASE WHEN tokens BETWEEN 0 AND a.token_limit THEN tokens END
    );
END;
$$;

REVOKE ALL ON FUNCTION craft_queue.finish_failed_attempt(text, text, bigint, bigint) FROM PUBLIC;

CREATE OR REPLACE FUNCTION craft_queue.work_audit(order_id text) RETURNS jsonb
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT jsonb_build_object(
        'work', craft_queue.status(w.id),
        'attempts', COALESCE((
            SELECT jsonb_agg(to_jsonb(a) ORDER BY a.generation DESC) FROM (
                SELECT generation, runner_id, started_at, expires_at, token_limit, reported_tokens, outcome,
                       claude_approval_id, worker_stop_reported_at
                FROM craft_queue.attempts AS a WHERE a.work_id = w.id
                ORDER BY generation DESC LIMIT 64
            ) AS a
        ), '[]'::jsonb),
        'verifications', COALESCE((
            SELECT jsonb_agg(to_jsonb(v) ORDER BY v.sequence DESC) FROM (
                SELECT sequence, generation, head, digest, verifier, passed, evidence, observed_at
                FROM craft_queue.verifications AS v WHERE v.work_id = w.id
                ORDER BY sequence DESC LIMIT 64
            ) AS v
        ), '[]'::jsonb),
        'publications', COALESCE((
            SELECT jsonb_agg(to_jsonb(p) ORDER BY p.generation DESC) FROM (
                SELECT action_key, generation, head, state, result_url
                FROM craft_queue.publications AS p WHERE p.work_id = w.id
                ORDER BY generation DESC LIMIT 64
            ) AS p
        ), '[]'::jsonb)
    )
    FROM craft_queue.work AS w WHERE w.id = order_id;
$$;

COMMIT;
