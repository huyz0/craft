BEGIN;

CREATE FUNCTION craft_queue.standing_delegation_current(order_id text, now_at timestamptz)
RETURNS boolean
LANGUAGE sql STABLE SET search_path = pg_catalog AS $$
    SELECT NOT EXISTS (
        SELECT 1 FROM craft_queue.standing_work AS binding
        LEFT JOIN craft_queue.standing_rules AS rule ON rule.id = binding.rule_id
        LEFT JOIN craft_queue.standing_observations AS observation
          ON observation.rule_id = binding.rule_id AND observation.issue_key = binding.issue_key
        WHERE binding.work_id = order_id AND (
            binding.blocked_at IS NULL AND rule.enabled AND NOT observation.reassessment_required
            AND (observation.jira_source_digest IS NULL OR observation.jira_source_approved)
            AND observation.source_read_at >= floor(extract(epoch FROM now_at))::bigint - 300
        ) IS NOT TRUE
    );
$$;

CREATE OR REPLACE FUNCTION craft_queue.standing_runner_allowed(order_id text, runner text, now_at timestamptz)
RETURNS boolean
LANGUAGE sql STABLE SET search_path = pg_catalog AS $$
    SELECT craft_queue.standing_delegation_current(order_id, now_at) AND NOT EXISTS (
        SELECT 1 FROM craft_queue.standing_work AS binding
        WHERE binding.work_id = order_id AND (
            binding.preferred_runner = runner
            OR (binding.fallback_runner = runner AND binding.fallback_at <= now_at)
        ) IS NOT TRUE
    );
$$;

CREATE OR REPLACE FUNCTION craft_queue.guard_publication_skill() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog AS $$
BEGIN
    IF NEW.state IN ('reserved', 'sending') AND NOT EXISTS (
        SELECT 1 FROM craft_queue.work AS w WHERE w.id = NEW.work_id
          AND craft_queue.team_skill_allowed(w.repository, w.payload)
          AND craft_queue.project_context_allowed(w.repository, w.payload)
          AND craft_queue.standing_delegation_current(w.id, clock_timestamp())
    ) THEN
        RAISE EXCEPTION 'publication requires current delegation, skill, and context';
    END IF;
    RETURN NEW;
END;
$$;

CREATE OR REPLACE FUNCTION craft_queue.renew(runner text, lease_work_id text, lease_generation bigint)
RETURNS timestamptz
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    settings craft_queue.control%ROWTYPE;
    now_at timestamptz;
    renewed timestamptz;
BEGIN
    SELECT * INTO STRICT settings FROM craft_queue.control FOR UPDATE;
    now_at := clock_timestamp();
    UPDATE craft_queue.attempts AS a
    SET expires_at = LEAST(now_at + make_interval(secs => settings.lease_seconds),
        (SELECT g.expires_at FROM craft_queue.claude_repository_approvals AS g WHERE g.grant_id = a.claude_approval_id))
    FROM craft_queue.work AS w, craft_queue.runner_repositories AS grant_row
    WHERE w.id = lease_work_id AND w.state = 'leased' AND w.generation = lease_generation
      AND a.work_id = w.id AND a.generation = w.generation AND a.runner_id = runner
      AND a.outcome = 'leased' AND a.expires_at > now_at
      AND grant_row.runner_id = runner AND grant_row.repository = w.repository
      AND (w.payload->>'agent_adapter' IS DISTINCT FROM 'claude' OR a.claude_approval_id IS NOT NULL)
      AND craft_queue.series_blocker(w.id) IS NULL
      AND craft_queue.standing_runner_allowed(w.id, runner, now_at)
    RETURNING a.expires_at INTO renewed;
    RETURN renewed;
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
    IF NOT FOUND OR tokens > attempt.token_limit THEN RETURN false; END IF;
    IF selected.state = 'candidate' AND attempt.outcome = 'candidate' THEN
        RETURN selected.candidate = commit_id AND selected.candidate_evidence = evidence_ref
             AND attempt.reported_tokens IS NOT DISTINCT FROM tokens;
    END IF;
    IF selected.state <> 'leased' OR attempt.outcome <> 'leased' OR attempt.expires_at <= now_at
       OR NOT craft_queue.standing_runner_allowed(selected.id, runner, now_at) THEN
        RETURN false;
    END IF;
    UPDATE craft_queue.attempts AS a SET outcome = 'candidate', reported_tokens = tokens
    WHERE a.work_id = selected.id AND a.generation = selected.generation;
    UPDATE craft_queue.work AS w SET state = 'candidate', candidate = commit_id, candidate_evidence = evidence_ref
    WHERE w.id = selected.id;
    RETURN true;
END;
$$;

CREATE OR REPLACE FUNCTION craft_queue.submit_candidate_pack(
    runner text, lease_work_id text, lease_generation bigint, commit_id text, pack bytea, tokens bigint
) RETURNS text
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    reference text;
BEGIN
    IF pack IS NULL OR octet_length(pack) NOT BETWEEN 1 AND 524288
       OR substring(pack FROM 1 FOR 4) <> decode('5041434b', 'hex') THEN
        RAISE EXCEPTION 'invalid candidate pack';
    END IF;
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    reference := 'sha256:' || encode(sha256(pack), 'hex');
    IF NOT craft_queue.standing_runner_allowed(lease_work_id, runner, clock_timestamp())
       AND NOT EXISTS (
           SELECT 1 FROM craft_queue.candidate_packs AS saved
           WHERE saved.work_id = lease_work_id AND saved.generation = lease_generation
             AND saved.digest = reference
       ) THEN
        RETURN NULL;
    END IF;
    IF NOT craft_queue.submit_candidate(
        runner, lease_work_id, lease_generation, commit_id, reference, tokens
    ) THEN
        RETURN NULL;
    END IF;
    INSERT INTO craft_queue.candidate_packs (work_id, generation, digest, payload)
    VALUES (lease_work_id, lease_generation, reference, pack)
    ON CONFLICT (work_id, generation) DO NOTHING;
    RETURN reference;
END;
$$;

REVOKE ALL ON FUNCTION craft_queue.standing_delegation_current(text, timestamptz) FROM PUBLIC;

COMMIT;
