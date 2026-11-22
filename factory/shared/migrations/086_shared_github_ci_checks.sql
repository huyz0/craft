BEGIN;

CREATE TABLE craft_queue.github_ci_checks (
    verification_sequence bigint PRIMARY KEY REFERENCES craft_queue.verifications(sequence),
    work_id text NOT NULL REFERENCES craft_queue.work(id),
    publication_key text NOT NULL REFERENCES craft_queue.publications(action_key),
    verifier text NOT NULL,
    proof jsonb NOT NULL,
    recorded_at timestamptz NOT NULL DEFAULT clock_timestamp()
);
CREATE INDEX github_ci_checks_by_work ON craft_queue.github_ci_checks(work_id, verification_sequence DESC);

CREATE FUNCTION craft_queue.draft_before_native_ci(
    order_id text, candidate_generation bigint, candidate_head text, review_sequence bigint
) RETURNS text
LANGUAGE sql STABLE SET search_path = pg_catalog AS $$
    SELECT p.action_key FROM craft_queue.work AS w
    JOIN craft_queue.publications AS p ON p.work_id = w.id AND p.generation = w.generation
        AND p.head = w.candidate AND p.pack_digest = w.candidate_evidence
    JOIN LATERAL (
        SELECT v.* FROM craft_queue.verifications AS v WHERE v.work_id = w.id AND v.generation = w.generation
        ORDER BY v.sequence DESC LIMIT 1
    ) AS v ON v.sequence = review_sequence AND v.passed
        AND v.head = w.candidate AND v.digest = w.candidate_evidence
    WHERE w.id = order_id AND w.state = 'candidate' AND w.generation = candidate_generation
      AND w.candidate = candidate_head AND p.state = 'confirmed' AND p.result_url IS NOT NULL
      AND craft_queue.series_blocker(w.id) IS NULL;
$$;

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
    WHERE w.id = order_id AND (w.payload->>'ci_policy_digest' IS NULL OR EXISTS (
        SELECT 1 FROM craft_queue.github_ci_checks AS c
        WHERE c.work_id = order_id AND c.verification_sequence = review_sequence
          AND c.publication_key = draft.action_key
    ));
$$;

CREATE FUNCTION craft_queue.record_github_ci_checks(
    actor_id text, order_id text, candidate_generation bigint, candidate_head text, pack_pin text,
    review_sequence bigint, observed jsonb
) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    prior craft_queue.github_ci_checks%ROWTYPE;
    binding record;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    SELECT c.* INTO prior FROM craft_queue.github_ci_checks AS c
    JOIN craft_queue.publications AS p ON p.action_key = c.publication_key
    WHERE c.verification_sequence = review_sequence AND c.work_id = order_id
      AND p.generation = candidate_generation AND p.head = candidate_head AND p.pack_digest = pack_pin;
    IF FOUND THEN
        IF observed IS NULL THEN RETURN jsonb_build_object('recorded', to_jsonb(prior)); END IF;
        IF prior.verifier = actor_id AND prior.proof = observed THEN RETURN to_jsonb(prior); END IF;
        RETURN NULL;
    END IF;

    SELECT p.action_key, p.base, p.result_url, p.base_branch, p.repository, w.payload INTO binding
    FROM craft_queue.work AS w
    JOIN craft_queue.attempts AS a ON a.work_id = w.id AND a.generation = w.generation
    JOIN craft_queue.publications AS p ON p.action_key =
        craft_queue.draft_before_native_ci(order_id, candidate_generation, candidate_head, review_sequence)
    WHERE w.id = order_id AND w.candidate_evidence = pack_pin AND a.runner_id <> actor_id
      AND w.payload->>'ci_policy_digest' ~ '^sha256:[a-f0-9]{64}$'
      AND w.payload->'ci_failure' ? 'run_attempt'
      AND p.result_url ~ ('^https://github\.com/' || p.repository || '/pull/[1-9][0-9]*$')
      AND NOT EXISTS (SELECT 1 FROM craft_queue.recoveries AS r WHERE r.finished_at IS NULL)
      AND NOT EXISTS (SELECT 1 FROM craft_queue.publications AS active WHERE active.work_id = w.id
                      AND active.state IN ('reserved', 'sending', 'uncertain'));
    IF NOT FOUND THEN RETURN NULL; END IF;
    IF observed IS NULL THEN
        RETURN jsonb_build_object('work_order', binding.payload, 'repository', binding.repository,
            'base', binding.base, 'head', candidate_head, 'base_branch', binding.base_branch,
            'pull_request', binding.result_url, 'publication_key', binding.action_key);
    END IF;
    IF observed->>'publication_key' IS DISTINCT FROM binding.action_key
        OR observed->>'pull_request' IS DISTINCT FROM binding.result_url
        OR observed->>'head' IS DISTINCT FROM candidate_head
        OR observed->>'policy_digest' IS DISTINCT FROM binding.payload->>'ci_policy_digest'
        OR jsonb_typeof(observed->'checks') IS DISTINCT FROM 'array' THEN RETURN NULL; END IF;
    IF jsonb_array_length(observed->'checks') NOT BETWEEN 1 AND 16 OR EXISTS (
        SELECT 1 FROM jsonb_array_elements(observed->'checks') AS items(check_run)
        WHERE COALESCE(check_run->>'id', '') !~ '^[1-9][0-9]{0,19}$'
           OR COALESCE(check_run->>'app_id', '') !~ '^[1-9][0-9]{0,19}$'
           OR length(btrim(COALESCE(check_run->>'name', ''))) NOT BETWEEN 1 AND 200
    ) OR NOT EXISTS (
        SELECT 1 FROM jsonb_array_elements(observed->'checks') AS items(check_run)
        WHERE check_run->>'name' = binding.payload #>> '{ci_failure,job_name}'
          AND check_run->>'app_id' = binding.payload #>> '{ci_failure,check_app_id}'
    ) THEN RETURN NULL; END IF;

    INSERT INTO craft_queue.github_ci_checks(verification_sequence, work_id, publication_key, verifier, proof)
    VALUES (review_sequence, order_id, binding.action_key, actor_id, observed) RETURNING * INTO prior;
    RETURN to_jsonb(prior);
END;
$$;

CREATE FUNCTION craft_queue.github_ci_checks_for(order_id text) RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT COALESCE(jsonb_agg(to_jsonb(receipt) ORDER BY receipt.verification_sequence DESC), '[]'::jsonb)
    FROM (SELECT c.* FROM craft_queue.github_ci_checks AS c WHERE c.work_id = order_id
          ORDER BY c.verification_sequence DESC LIMIT 20) AS receipt;
$$;

CREATE FUNCTION craft_queue.require_github_ci_before_merge() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog AS $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM craft_queue.shadow_merge_receipts AS r
        JOIN craft_queue.work AS w ON w.id = r.work_id
        WHERE r.id = NEW.receipt_id AND w.payload->>'ci_policy_digest' IS NOT NULL
          AND r.publication_key IS DISTINCT FROM
              craft_queue.current_review_draft(r.work_id, r.generation, r.head, r.verification_sequence)
    ) THEN
        RAISE EXCEPTION 'record current GitHub CI evidence before observing the merge';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER require_github_ci_before_merge BEFORE INSERT ON craft_queue.github_shadow_merges
FOR EACH ROW EXECUTE FUNCTION craft_queue.require_github_ci_before_merge();

REVOKE ALL ON FUNCTION craft_queue.draft_before_native_ci(text,bigint,text,bigint) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.record_github_ci_checks(text,text,bigint,text,text,bigint,jsonb) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.github_ci_checks_for(text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.require_github_ci_before_merge() FROM PUBLIC;

COMMIT;
