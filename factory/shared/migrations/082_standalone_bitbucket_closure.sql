BEGIN;

CREATE TABLE craft_queue.bitbucket_work_merges (
    publication_key text PRIMARY KEY REFERENCES craft_queue.publications(action_key),
    work_id text NOT NULL REFERENCES craft_queue.work(id),
    verifier text NOT NULL,
    observation jsonb NOT NULL,
    recorded_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    closed_by text,
    closure_evidence text,
    closed_at timestamptz,
    closure_verification_sequence bigint REFERENCES craft_queue.verifications(sequence)
);
CREATE INDEX bitbucket_work_merges_by_work ON craft_queue.bitbucket_work_merges(work_id, recorded_at DESC);
CREATE UNIQUE INDEX bitbucket_one_closure_per_work ON craft_queue.bitbucket_work_merges(work_id)
    WHERE closed_at IS NOT NULL;

CREATE FUNCTION craft_queue.bitbucket_closure_target(order_id text, expected_head text)
RETURNS TABLE (publication_key text, repository text, base text, base_branch text, pull_request text,
               verification_sequence bigint, runner_id text, checked_at timestamptz)
LANGUAGE sql STABLE SET search_path = pg_catalog AS $$
    SELECT p.action_key, p.repository, p.base, p.base_branch, p.result_url, latest.sequence,
           a.runner_id, published_verdict.observed_at
    FROM craft_queue.work AS w
    JOIN craft_queue.attempts AS a ON a.work_id = w.id AND a.generation = w.generation
    JOIN LATERAL (
        SELECT v.sequence FROM craft_queue.verifications AS v
        WHERE v.work_id = w.id AND v.generation = w.generation
        ORDER BY v.sequence DESC LIMIT 1
    ) AS latest ON true
    JOIN craft_queue.publications AS p ON p.action_key =
        craft_queue.current_review_draft(w.id, w.generation, expected_head, latest.sequence)
    JOIN craft_queue.verifications AS published_verdict ON published_verdict.sequence = p.verification_sequence
    WHERE w.id = order_id AND w.candidate = expected_head
      AND p.result_url ~ ('^https://bitbucket\.org/' || p.repository || '/pull-requests/[1-9][0-9]*$')
      AND p.repository ~ '^[a-z0-9_-]+/[a-z0-9_-]+$'
      AND NOT EXISTS (SELECT 1 FROM craft_queue.series_members AS s WHERE s.work_id = w.id)
      AND NOT EXISTS (SELECT 1 FROM craft_queue.recoveries AS r WHERE r.finished_at IS NULL)
      AND NOT EXISTS (SELECT 1 FROM craft_queue.publications AS active WHERE active.work_id = w.id
                      AND active.state IN ('reserved', 'sending', 'uncertain'));
$$;

CREATE FUNCTION craft_queue.confirm_bitbucket_work_merge(
    actor_id text, order_id text, expected_head text, publication text, observed jsonb
) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    binding record;
    stored craft_queue.bitbucket_work_merges%ROWTYPE;
    merge_time bigint;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    IF publication IS NULL AND observed IS NULL THEN
        SELECT m.* INTO stored FROM craft_queue.bitbucket_work_merges AS m
        WHERE m.work_id = order_id AND m.closed_at IS NOT NULL AND m.observation->>'head' = expected_head;
        IF FOUND THEN RETURN jsonb_build_object('head', expected_head, 'recorded', to_jsonb(stored)); END IF;
    END IF;
    IF publication IS NOT NULL AND observed IS NOT NULL THEN
        SELECT m.* INTO stored FROM craft_queue.bitbucket_work_merges AS m
        WHERE m.publication_key = publication AND m.work_id = order_id;
        IF FOUND THEN
            IF stored.verifier = actor_id AND stored.observation = observed
                AND stored.observation->>'head' = expected_head THEN RETURN to_jsonb(stored); END IF;
            RETURN NULL;
        END IF;
    END IF;
    SELECT t.* INTO binding FROM craft_queue.bitbucket_closure_target(order_id, expected_head) AS t;
    IF NOT FOUND OR actor_id = binding.runner_id THEN RETURN NULL; END IF;
    IF publication IS NULL AND observed IS NULL THEN
        SELECT m.* INTO stored FROM craft_queue.bitbucket_work_merges AS m
        WHERE m.publication_key = binding.publication_key;
        RETURN jsonb_build_object('publication_key', binding.publication_key, 'repository', binding.repository,
            'head', expected_head, 'base', binding.base, 'base_branch', binding.base_branch,
            'pull_request', binding.pull_request, 'recorded', CASE WHEN FOUND THEN to_jsonb(stored) END);
    END IF;
    IF publication IS DISTINCT FROM binding.publication_key
        OR COALESCE(observed->>'merged_at', '') !~ '^[1-9][0-9]{0,11}$' THEN RETURN NULL; END IF;
    merge_time := (observed->>'merged_at')::bigint;
    IF merge_time < floor(extract(epoch FROM binding.checked_at))
        OR merge_time > floor(extract(epoch FROM clock_timestamp()))
        OR observed->>'pull_request' IS DISTINCT FROM binding.pull_request
        OR observed->>'head' IS DISTINCT FROM expected_head
        OR observed->>'pinned_base' IS DISTINCT FROM binding.base
        OR observed->>'destination_branch' IS DISTINCT FROM binding.base_branch
        OR COALESCE(observed->>'repository_uuid', '') !~ '^[a-f0-9]{8}(-[a-f0-9]{4}){3}-[a-f0-9]{12}$'
        OR COALESCE(observed->>'actor', '') !~ '^bitbucket:user:[a-f0-9]{8}(-[a-f0-9]{4}){3}-[a-f0-9]{12}$'
        OR COALESCE(observed->>'result_commit', '') !~ '^([a-f0-9]{40}|[a-f0-9]{64})$'
        OR length(observed->>'result_commit') <> length(expected_head)
        OR observed->>'result_commit' = binding.base
        OR observed->>'reference' IS DISTINCT FROM ('https://bitbucket.org/' || binding.repository
            || '/commits/' || (observed->>'result_commit'))
        THEN RETURN NULL; END IF;

    INSERT INTO craft_queue.bitbucket_work_merges(publication_key, work_id, verifier, observation)
    VALUES (publication, order_id, actor_id, observed) RETURNING * INTO stored;
    RETURN to_jsonb(stored);
END;
$$;

CREATE FUNCTION craft_queue.close_bitbucket_work(
    actor_id text, order_id text, publication text, expected_head text, evidence_ref text
) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    stored craft_queue.bitbucket_work_merges%ROWTYPE;
    binding record;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    SELECT m.* INTO stored FROM craft_queue.bitbucket_work_merges AS m
    WHERE m.publication_key = publication AND m.work_id = order_id AND m.observation->>'head' = expected_head;
    IF NOT FOUND THEN RETURN NULL; END IF;
    IF stored.closed_at IS NOT NULL THEN
        IF stored.closed_by = actor_id AND stored.closure_evidence = evidence_ref THEN RETURN to_jsonb(stored); END IF;
        RETURN NULL;
    END IF;
    IF length(btrim(COALESCE(evidence_ref, ''))) NOT BETWEEN 1 AND 500
        OR actor_id = stored.verifier THEN RETURN NULL; END IF;
    SELECT t.* INTO binding FROM craft_queue.bitbucket_closure_target(order_id, expected_head) AS t;
    IF NOT FOUND OR publication IS DISTINCT FROM binding.publication_key THEN RETURN NULL; END IF;

    UPDATE craft_queue.bitbucket_work_merges AS m
    SET closed_by = actor_id, closure_evidence = evidence_ref, closed_at = clock_timestamp(),
        closure_verification_sequence = binding.verification_sequence
    WHERE m.publication_key = publication RETURNING * INTO stored;
    UPDATE craft_queue.work AS w SET state = 'released' WHERE w.id = order_id;
    RETURN to_jsonb(stored);
END;
$$;

ALTER FUNCTION craft_queue.status(text) RENAME TO status_before_bitbucket_closure;
CREATE FUNCTION craft_queue.status(work_id text) RETURNS jsonb
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT craft_queue.status_before_bitbucket_closure($1) ||
        COALESCE((SELECT CASE WHEN m.closed_at IS NULL THEN jsonb_build_object('bitbucket_merge', to_jsonb(m))
                              ELSE jsonb_build_object('state', 'merged', 'independent_checks_passed', true,
                                                      'bitbucket_merge', to_jsonb(m)) END
                  FROM craft_queue.bitbucket_work_merges AS m JOIN craft_queue.work AS w ON w.id = m.work_id
                  JOIN craft_queue.publications AS p ON p.action_key = m.publication_key
                  WHERE m.work_id = $1 AND (m.closed_at IS NOT NULL OR
                        (w.state = 'candidate' AND p.generation = w.generation AND p.head = w.candidate))
                  ORDER BY (m.closed_at IS NOT NULL) DESC, m.recorded_at DESC LIMIT 1), '{}'::jsonb);
$$;

CREATE OR REPLACE FUNCTION craft_queue.guard_github_work_closure() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog AS $$
BEGIN
    IF OLD.state = 'released' AND NEW.state <> 'released' AND (
        EXISTS (SELECT 1 FROM craft_queue.github_work_closures AS c WHERE c.work_id = OLD.id)
        OR EXISTS (SELECT 1 FROM craft_queue.bitbucket_work_merges AS m WHERE m.work_id = OLD.id AND m.closed_at IS NOT NULL)
    ) THEN RAISE EXCEPTION 'standalone work has a confirmed merge'; END IF;
    RETURN NEW;
END;
$$;

CREATE OR REPLACE FUNCTION craft_queue.withdraw_series_on_restore() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog AS $$
BEGIN
    UPDATE craft_queue.series_observations
    SET withdrawn_at = clock_timestamp(), withdrawal_evidence = 'Restore recovery ' || NEW.epoch::text
    WHERE withdrawn_at IS NULL;
    UPDATE craft_queue.work AS w
    SET state = CASE WHEN craft_queue.team_skill_allowed(w.repository, w.payload)
        AND craft_queue.project_context_allowed(w.repository, w.payload) THEN 'candidate' ELSE 'cancelled' END,
        cancellation_reason = CASE WHEN craft_queue.team_skill_allowed(w.repository, w.payload)
        AND craft_queue.project_context_allowed(w.repository, w.payload) THEN NULL
        ELSE 'Release needs fresh observation after restore and required context or skill was retired' END
    WHERE w.state = 'released' AND EXISTS (SELECT 1 FROM craft_queue.series_members AS s WHERE s.work_id = w.id);
    RETURN NEW;
END;
$$;

REVOKE ALL ON FUNCTION craft_queue.status(text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.bitbucket_closure_target(text, text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.confirm_bitbucket_work_merge(text, text, text, text, jsonb) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.close_bitbucket_work(text, text, text, text, text) FROM PUBLIC;

COMMIT;
