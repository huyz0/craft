BEGIN;

CREATE TABLE craft_queue.bitbucket_ci_checks (
    verification_sequence bigint PRIMARY KEY REFERENCES craft_queue.verifications(sequence),
    work_id text NOT NULL REFERENCES craft_queue.work(id),
    publication_key text NOT NULL REFERENCES craft_queue.publications(action_key),
    verifier text NOT NULL,
    proof jsonb NOT NULL,
    recorded_at timestamptz NOT NULL DEFAULT clock_timestamp()
);
CREATE INDEX bitbucket_ci_checks_by_work ON craft_queue.bitbucket_ci_checks(work_id, verification_sequence DESC);

CREATE OR REPLACE FUNCTION craft_queue.current_review_draft(
    order_id text, candidate_generation bigint, candidate_head text, review_sequence bigint
) RETURNS text
LANGUAGE sql STABLE SET search_path = pg_catalog AS $$
    SELECT draft.action_key
    FROM craft_queue.work AS w
    CROSS JOIN LATERAL (
        SELECT craft_queue.draft_before_native_ci(order_id, candidate_generation, candidate_head, review_sequence)
            AS action_key
    ) AS draft
    WHERE w.id = order_id AND (
        (w.payload->>'ci_failure' IS NULL AND w.payload->>'ci_policy_digest' IS NULL)
        OR (w.payload->'ci_failure' ? 'run_attempt' AND EXISTS (
        SELECT 1 FROM craft_queue.github_ci_checks AS c
        WHERE c.work_id = order_id AND c.verification_sequence = review_sequence
          AND c.publication_key = draft.action_key
    )) OR (w.payload->'ci_failure' ? 'step_uuid' AND EXISTS (
        SELECT 1 FROM craft_queue.bitbucket_ci_checks AS c
        WHERE c.work_id = order_id AND c.verification_sequence = review_sequence
          AND c.publication_key = draft.action_key
    )));
$$;

CREATE FUNCTION craft_queue.record_bitbucket_ci_checks(
    actor_id text, order_id text, candidate_generation bigint, candidate_head text, pack_pin text,
    review_sequence bigint, observed jsonb
) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    prior craft_queue.bitbucket_ci_checks%ROWTYPE;
    binding record;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    SELECT c.* INTO prior FROM craft_queue.bitbucket_ci_checks AS c
    JOIN craft_queue.publications AS p ON p.action_key = c.publication_key
    WHERE c.verification_sequence = review_sequence AND c.work_id = order_id
      AND p.generation = candidate_generation AND p.head = candidate_head AND p.pack_digest = pack_pin;
    IF FOUND THEN
        IF observed IS NULL THEN RETURN jsonb_build_object('recorded', to_jsonb(prior)); END IF;
        IF prior.verifier = actor_id AND prior.proof = observed THEN RETURN to_jsonb(prior); END IF;
        RETURN NULL;
    END IF;

    SELECT p.action_key, p.base, p.result_url, p.base_branch, p.source_branch, p.repository, w.payload INTO binding
    FROM craft_queue.work AS w
    JOIN craft_queue.attempts AS a ON a.work_id = w.id AND a.generation = w.generation
    JOIN craft_queue.publications AS p ON p.action_key =
        craft_queue.draft_before_native_ci(order_id, candidate_generation, candidate_head, review_sequence)
    WHERE w.id = order_id AND w.candidate_evidence = pack_pin AND a.runner_id <> actor_id
      AND w.payload->>'ci_policy_digest' ~ '^sha256:[a-f0-9]{64}$'
      AND w.payload->'ci_failure' ? 'step_uuid'
      AND w.payload #>> '{ci_failure,repository}' = p.repository
      AND w.payload #>> '{ci_failure,branch}' = p.base_branch
      AND split_part(p.result_url, '/pull-requests/', 2) ~ '^[1-9][0-9]{0,19}$'
      AND p.result_url = 'https://bitbucket.org/' || p.repository || '/pull-requests/'
          || split_part(p.result_url, '/pull-requests/', 2)
      AND NOT EXISTS (SELECT 1 FROM craft_queue.recoveries AS r WHERE r.finished_at IS NULL)
      AND NOT EXISTS (SELECT 1 FROM craft_queue.publications AS active WHERE active.work_id = w.id
                      AND active.state IN ('reserved', 'sending', 'uncertain'));
    IF NOT FOUND THEN RETURN NULL; END IF;
    IF observed IS NULL THEN
        RETURN jsonb_build_object('work_order', binding.payload, 'repository', binding.repository,
            'base', binding.base, 'head', candidate_head, 'base_branch', binding.base_branch,
            'pull_request', binding.result_url, 'publication_key', binding.action_key,
            'source_branch', binding.source_branch);
    END IF;
    IF observed->>'publication_key' IS DISTINCT FROM binding.action_key
        OR observed->>'pull_request' IS DISTINCT FROM binding.result_url
        OR observed->>'head' IS DISTINCT FROM candidate_head
        OR observed->>'policy_digest' IS DISTINCT FROM binding.payload->>'ci_policy_digest'
        OR observed->>'base' IS DISTINCT FROM binding.base
        OR observed->>'source_branch' IS DISTINCT FROM binding.source_branch
        OR COALESCE(observed->>'build_number', '') !~ '^[1-9][0-9]{0,19}$'
        OR observed->>'pipeline' IS DISTINCT FROM ('https://bitbucket.org/' || binding.repository
            || '/pipelines/results/' || (observed->>'build_number'))
        OR COALESCE(observed->>'pipeline_uuid', '') !~ '^[a-f0-9]{8}(-[a-f0-9]{4}){3}-[a-f0-9]{12}$'
        OR jsonb_typeof(observed->'checks') IS DISTINCT FROM 'array' THEN RETURN NULL; END IF;
    IF jsonb_array_length(observed->'checks') NOT BETWEEN 1 AND 16 OR EXISTS (
        SELECT 1 FROM jsonb_array_elements(observed->'checks') AS items(check_run)
        WHERE COALESCE(check_run->>'id', '') !~ '^[a-f0-9]{8}(-[a-f0-9]{4}){3}-[a-f0-9]{12}$'
           OR length(btrim(COALESCE(check_run->>'name', ''))) NOT BETWEEN 1 AND 200
    ) OR EXISTS (
        SELECT 1 FROM jsonb_array_elements(observed->'checks') AS items(check_run)
        GROUP BY check_run->>'name' HAVING count(*) > 1
    ) OR EXISTS (
        SELECT 1 FROM jsonb_array_elements(observed->'checks') AS items(check_run)
        GROUP BY check_run->>'id' HAVING count(*) > 1
    ) OR NOT EXISTS (
        SELECT 1 FROM jsonb_array_elements(observed->'checks') AS items(check_run)
        WHERE check_run->>'name' = binding.payload #>> '{ci_failure,step_name}'
    ) THEN RETURN NULL; END IF;

    INSERT INTO craft_queue.bitbucket_ci_checks(verification_sequence, work_id, publication_key, verifier, proof)
    VALUES (review_sequence, order_id, binding.action_key, actor_id, observed) RETURNING * INTO prior;
    RETURN to_jsonb(prior);
END;
$$;

CREATE FUNCTION craft_queue.bitbucket_ci_checks_for(order_id text) RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT COALESCE(jsonb_agg(to_jsonb(receipt) ORDER BY receipt.verification_sequence DESC), '[]'::jsonb)
    FROM (SELECT c.* FROM craft_queue.bitbucket_ci_checks AS c WHERE c.work_id = order_id
          ORDER BY c.verification_sequence DESC LIMIT 20) AS receipt;
$$;

CREATE OR REPLACE FUNCTION craft_queue.hold_obsolete_ci(runner text, order_id text, lease_generation bigint, reason_code text)
RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    hold craft_queue.ci_preflight_holds%ROWTYPE;
    now_at timestamptz;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    SELECT h.* INTO hold FROM craft_queue.ci_preflight_holds AS h
    JOIN craft_queue.work AS w ON w.id = h.work_id
    JOIN craft_queue.runner_repositories AS enrollment
      ON enrollment.repository = w.repository AND enrollment.runner_id = runner
    WHERE h.work_id = order_id AND h.generation = lease_generation;
    IF FOUND THEN
        IF hold.runner_id = runner AND hold.reason = reason_code THEN RETURN to_jsonb(hold); END IF;
        RETURN NULL;
    END IF;
    IF COALESCE(reason_code, '') NOT IN ('branch_moved', 'workflow_superseded') THEN RETURN NULL; END IF;
    now_at := clock_timestamp();
    UPDATE craft_queue.attempts AS a SET expires_at = now_at, outcome = 'expired', reported_tokens = 0
    FROM craft_queue.work AS w
    WHERE w.id = order_id AND a.work_id = w.id AND a.generation = lease_generation
      AND craft_queue.runner_context_allowed(w, a, runner, lease_generation)
      AND a.expires_at > now_at AND a.profile_started_at IS NULL AND a.reported_tokens IS NULL
      AND w.payload->>'ci_policy_digest' ~ '^sha256:[a-f0-9]{64}$'
      AND (w.payload->'ci_failure' ? 'run_attempt' OR w.payload->'ci_failure' ? 'step_uuid')
      AND NOT EXISTS (SELECT 1 FROM craft_queue.recoveries AS r WHERE r.finished_at IS NULL);
    IF NOT FOUND THEN RETURN NULL; END IF;

    INSERT INTO craft_queue.ci_preflight_holds(work_id, generation, runner_id, reason)
    VALUES (order_id, lease_generation, runner, reason_code) RETURNING * INTO hold;
    UPDATE craft_queue.work AS w SET state = 'blocked' WHERE w.id = order_id;
    RETURN to_jsonb(hold);
END;
$$;

REVOKE ALL ON FUNCTION craft_queue.record_bitbucket_ci_checks(text,text,bigint,text,text,bigint,jsonb) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.bitbucket_ci_checks_for(text) FROM PUBLIC;

COMMIT;
