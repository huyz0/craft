BEGIN;

CREATE TABLE craft_queue.standing_rules (
    id text PRIMARY KEY CHECK (id ~ '^[A-Za-z0-9_-]{1,100}$'),
    intake_id text NOT NULL,
    source text NOT NULL,
    project_key text NOT NULL CHECK (project_key ~ '^[A-Z][A-Z0-9]{0,30}$'),
    repository text NOT NULL,
    human_account text NOT NULL,
    human_owner text NOT NULL CHECK (length(btrim(human_owner)) BETWEEN 1 AND 200),
    required_label text NOT NULL CHECK (length(btrim(required_label)) BETWEEN 1 AND 100),
    eligible_status text NOT NULL CHECK (length(btrim(eligible_status)) BETWEEN 1 AND 100),
    allowed_paths text[] NOT NULL CHECK (cardinality(allowed_paths) BETWEEN 1 AND 50),
    preferred_runner text NOT NULL,
    fallback_runner text,
    fallback_seconds integer CHECK (fallback_seconds BETWEEN 1 AND 900),
    token_limit bigint NOT NULL CHECK (token_limit > 0),
    approval_reference text NOT NULL CHECK (length(btrim(approval_reference)) BETWEEN 1 AND 500),
    enabled boolean NOT NULL DEFAULT false,
    CHECK ((fallback_runner IS NULL) = (fallback_seconds IS NULL)),
    CHECK (fallback_runner IS DISTINCT FROM preferred_runner)
);

CREATE TABLE craft_queue.standing_observations (
    rule_id text REFERENCES craft_queue.standing_rules(id),
    issue_key text,
    source_revision bigint NOT NULL,
    source_read_at bigint NOT NULL,
    fingerprint bytea NOT NULL,
    reassessment_required boolean NOT NULL DEFAULT false,
    reassessment_reference text CHECK (length(btrim(reassessment_reference)) BETWEEN 1 AND 500),
    PRIMARY KEY (rule_id, issue_key)
);

CREATE TABLE craft_queue.standing_work (
    work_id text PRIMARY KEY REFERENCES craft_queue.work(id),
    rule_id text NOT NULL REFERENCES craft_queue.standing_rules(id),
    issue_key text NOT NULL,
    source_revision bigint NOT NULL,
    human_account text NOT NULL,
    approval_reference text NOT NULL,
    intent_approval_reference text,
    preferred_runner text NOT NULL,
    fallback_runner text,
    fallback_at timestamptz,
    blocked_at timestamptz
);
CREATE INDEX standing_work_issue ON craft_queue.standing_work(issue_key, rule_id);

CREATE FUNCTION craft_queue.fence_standing_work(policy_id text, only_issue text, reason text)
RETURNS void
LANGUAGE sql SET search_path = pg_catalog AS $$
    WITH blocked AS (
        UPDATE craft_queue.standing_work AS binding SET blocked_at = clock_timestamp()
        WHERE binding.rule_id = policy_id AND (only_issue IS NULL OR binding.issue_key = only_issue)
          AND binding.blocked_at IS NULL
        RETURNING binding.work_id
    ), cancelled AS (
        UPDATE craft_queue.work AS w SET state = 'cancelled', cancellation_reason = reason
        FROM blocked WHERE w.id = blocked.work_id AND w.state IN ('queued', 'leased', 'candidate')
          AND NOT EXISTS (
              SELECT 1 FROM craft_queue.publications AS p
              WHERE p.work_id = w.id AND p.state IN ('reserved', 'sending', 'uncertain')
          )
        RETURNING w.id
    )
    UPDATE craft_queue.attempts AS a SET outcome = 'cancelled'
    FROM cancelled WHERE a.work_id = cancelled.id AND a.outcome = 'leased';
$$;

-- Administrative transition. Run as the queue owner, which owns the rule tables.
CREATE FUNCTION craft_queue.disable_standing_rule(policy_id text) RETURNS boolean
LANGUAGE plpgsql SET search_path = pg_catalog AS $$
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    UPDATE craft_queue.standing_rules SET enabled = false WHERE id = policy_id;
    IF NOT FOUND THEN RETURN false; END IF;
    PERFORM craft_queue.fence_standing_work(policy_id, NULL, 'Standing delegation revoked');
    RETURN true;
END;
$$;

CREATE FUNCTION craft_queue.standing_runner_allowed(order_id text, runner text, now_at timestamptz)
RETURNS boolean
LANGUAGE sql STABLE SET search_path = pg_catalog AS $$
    SELECT COALESCE((
        SELECT binding.blocked_at IS NULL AND rule.enabled
          AND observation.source_read_at >= floor(extract(epoch FROM now_at))::bigint - 300 AND (
            binding.preferred_runner = runner
            OR COALESCE(binding.fallback_runner = runner AND binding.fallback_at <= now_at, false)
        )
        FROM craft_queue.standing_work AS binding
        JOIN craft_queue.standing_rules AS rule ON rule.id = binding.rule_id
        JOIN craft_queue.standing_observations AS observation
          ON observation.rule_id = binding.rule_id AND observation.issue_key = binding.issue_key
        WHERE binding.work_id = order_id
    ), true);
$$;

CREATE FUNCTION craft_queue.approve_standing_intent(
    policy_id text, ticket text, reviewed_revision bigint, approval_ref text
) RETURNS boolean
LANGUAGE plpgsql SET search_path = pg_catalog AS $$
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    UPDATE craft_queue.standing_observations AS observation
    SET reassessment_required = false, reassessment_reference = approval_ref
    WHERE observation.rule_id = policy_id AND observation.issue_key = ticket
      AND observation.source_revision = reviewed_revision AND observation.reassessment_required
      AND length(btrim(approval_ref)) BETWEEN 1 AND 500;
    RETURN FOUND;
END;
$$;

CREATE OR REPLACE FUNCTION craft_queue.guard_publication_skill() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog AS $$
BEGIN
    IF NEW.state IN ('reserved', 'sending') AND NOT EXISTS (
        SELECT 1 FROM craft_queue.work AS w WHERE w.id = NEW.work_id
          AND craft_queue.team_skill_allowed(w.repository, w.payload)
          AND craft_queue.project_context_allowed(w.repository, w.payload)
          AND NOT EXISTS (
              SELECT 1 FROM craft_queue.standing_work AS binding
              JOIN craft_queue.standing_rules AS rule ON rule.id = binding.rule_id
              JOIN craft_queue.standing_observations AS observation
                ON observation.rule_id = binding.rule_id AND observation.issue_key = binding.issue_key
              WHERE binding.work_id = w.id AND (binding.blocked_at IS NOT NULL OR NOT rule.enabled
                OR observation.source_read_at < floor(extract(epoch FROM clock_timestamp()))::bigint - 300)
          )
    ) THEN
        RAISE EXCEPTION 'publication requires current delegation, skill, and context';
    END IF;
    RETURN NEW;
END;
$$;

CREATE FUNCTION craft_queue.observe_standing(
    identity text, policy_id text, snapshot jsonb, candidate_order jsonb
) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    settings craft_queue.control%ROWTYPE;
    policy craft_queue.standing_rules%ROWTYPE;
    previous craft_queue.standing_observations%ROWTYPE;
    existing record;
    ticket text := snapshot->>'issue';
    native_revision bigint := (snapshot->>'source_revision')::bigint;
    read_at bigint := (snapshot->>'observed_at')::bigint;
    now_at timestamptz;
    now_seconds bigint;
    observation_digest bytea;
    managed_order jsonb;
    new_work_id text;
BEGIN
    SELECT * INTO STRICT settings FROM craft_queue.control FOR UPDATE;
    SELECT * INTO policy FROM craft_queue.standing_rules AS rule
    WHERE rule.id = policy_id AND rule.intake_id = identity AND rule.source = snapshot->>'source';
    IF NOT FOUND THEN RETURN NULL; END IF;
    IF NOT policy.enabled THEN RETURN jsonb_build_object('state', 'disabled'); END IF;

    now_at := clock_timestamp();
    now_seconds := floor(extract(epoch FROM now_at))::bigint;
    IF COALESCE(native_revision, 0) < 1 OR read_at IS NULL OR read_at < now_seconds - 300
       OR read_at > now_seconds + 30 OR COALESCE(ticket, '') !~ ('^' || policy.project_key || '-[1-9][0-9]{0,17}$')
       OR jsonb_typeof(snapshot->'labels') IS DISTINCT FROM 'array' THEN
        RETURN jsonb_build_object('state', 'invalid_observation');
    END IF;
    observation_digest := sha256(convert_to(
        jsonb_build_array(snapshot - 'observed_at', candidate_order)::text, 'UTF8'));
    SELECT * INTO previous FROM craft_queue.standing_observations AS observation
    WHERE observation.rule_id = policy_id AND observation.issue_key = ticket;
    IF FOUND AND (native_revision < previous.source_revision OR (
        native_revision = previous.source_revision AND observation_digest <> previous.fingerprint
    )) THEN RETURN jsonb_build_object('state', 'stale_or_conflicting'); END IF;
    INSERT INTO craft_queue.standing_observations (rule_id, issue_key, source_revision, source_read_at, fingerprint)
    VALUES (policy_id, ticket, native_revision, read_at, observation_digest)
    ON CONFLICT (rule_id, issue_key) DO UPDATE
        SET source_revision = EXCLUDED.source_revision,
            source_read_at = greatest(standing_observations.source_read_at, EXCLUDED.source_read_at),
            fingerprint = EXCLUDED.fingerprint,
            reassessment_reference = CASE WHEN standing_observations.source_revision = EXCLUDED.source_revision
                THEN standing_observations.reassessment_reference ELSE NULL END;

    IF snapshot->>'assignee' IS DISTINCT FROM policy.human_account
       OR snapshot->>'status' IS DISTINCT FROM policy.eligible_status
       OR NOT (snapshot->'labels' ? policy.required_label) THEN
        PERFORM craft_queue.fence_standing_work(policy_id, ticket, 'Standing delegation no longer matches the ticket');
        RETURN jsonb_build_object('state', 'ineligible');
    END IF;
    IF previous.reassessment_required THEN RETURN jsonb_build_object('state', 'review_required'); END IF;
    managed_order := (candidate_order - 'id' - 'repository') || jsonb_build_object(
        'owner', policy.human_owner, 'issue', ticket);
    IF jsonb_typeof(candidate_order) IS DISTINCT FROM 'object'
       OR managed_order->'schema_version' IS DISTINCT FROM '1'::jsonb
       OR COALESCE(managed_order->>'base', '') !~ '^([a-f0-9]{40}|[a-f0-9]{64})$'
       OR jsonb_typeof(managed_order->'intent') IS DISTINCT FROM 'string'
       OR btrim(managed_order->>'intent') = ''
       OR jsonb_typeof(managed_order->'acceptance') IS DISTINCT FROM 'array'
       OR managed_order->'acceptance' = '[]'::jsonb
       OR jsonb_typeof(managed_order->'context') IS DISTINCT FROM 'array'
       OR managed_order->'context' = '[]'::jsonb
       OR COALESCE(btrim(managed_order#>>'{skill,location}'), '') = ''
       OR COALESCE(btrim(managed_order#>>'{skill,revision}'), '') = ''
       OR jsonb_typeof(managed_order->'scope') IS DISTINCT FROM 'array' THEN
        PERFORM craft_queue.fence_standing_work(policy_id, ticket, 'Standing work order is incomplete');
        RETURN jsonb_build_object('state', 'invalid_order');
    END IF;
    IF jsonb_array_length(managed_order->'scope') = 0 OR EXISTS (
        SELECT 1 FROM jsonb_array_elements(managed_order->'scope') AS path(value)
        WHERE jsonb_typeof(path.value) <> 'string'
          OR NOT COALESCE((path.value #>> '{}') = ANY(policy.allowed_paths), false)
    ) THEN
        PERFORM craft_queue.fence_standing_work(policy_id, ticket, 'Standing work order is outside approved scope');
        RETURN jsonb_build_object('state', 'scope_not_allowed');
    END IF;

    SELECT w.id, w.payload, binding.rule_id, binding.blocked_at INTO existing
    FROM craft_queue.standing_work AS binding
    JOIN craft_queue.work AS w ON w.id = binding.work_id
    JOIN craft_queue.standing_rules AS active_rule ON active_rule.id = binding.rule_id
    WHERE binding.issue_key = ticket AND w.repository = policy.repository
      AND active_rule.source = policy.source AND w.state <> 'cancelled'
    ORDER BY w.created_at LIMIT 1;
    IF FOUND THEN
        IF existing.rule_id <> policy_id OR existing.blocked_at IS NOT NULL THEN
            RETURN jsonb_build_object('state', 'existing_work');
        END IF;
        IF existing.payload - 'id' = managed_order THEN
            RETURN jsonb_build_object('state', 'already_admitted', 'work_id', existing.id);
        END IF;
        PERFORM craft_queue.fence_standing_work(policy_id, ticket, 'Standing work order changed and needs review');
        UPDATE craft_queue.standing_observations AS observation SET reassessment_required = true
        WHERE observation.rule_id = policy_id AND observation.issue_key = ticket;
        RETURN jsonb_build_object('state', 'review_required');
    END IF;

    IF settings.paused THEN RETURN jsonb_build_object('state', 'paused'); END IF;
    IF NOT EXISTS (
        SELECT 1 FROM craft_queue.runner_repositories AS enrolled
        WHERE enrolled.repository = policy.repository AND enrolled.runner_id = policy.preferred_runner
    ) OR (policy.fallback_runner IS NOT NULL AND NOT EXISTS (
        SELECT 1 FROM craft_queue.runner_repositories AS enrolled
        WHERE enrolled.repository = policy.repository AND enrolled.runner_id = policy.fallback_runner
    )) THEN RETURN jsonb_build_object('state', 'runner_unavailable'); END IF;

    IF (
        SELECT count(*) FROM craft_queue.work AS w
        LEFT JOIN craft_queue.standing_work AS binding ON binding.work_id = w.id
        WHERE w.state IN ('leased', 'candidate')
           OR (w.state = 'queued' AND (w.repair_from_generation IS NOT NULL OR binding.work_id IS NOT NULL))
    ) >= settings.review_limit THEN RETURN jsonb_build_object('state', 'review_capacity'); END IF;
    IF policy.token_limit > settings.token_budget - COALESCE((
        SELECT sum(COALESCE(a.reported_tokens, a.token_limit)) FROM craft_queue.attempts AS a
        WHERE a.started_at > now_at - interval '24 hours' OR (a.outcome = 'leased' AND a.expires_at > now_at)
    ), 0) THEN RETURN jsonb_build_object('state', 'token_capacity'); END IF;

    new_work_id := 'standing-' || encode(sha256(convert_to(
        jsonb_build_array(policy.id, ticket, native_revision)::text, 'UTF8')), 'hex');
    PERFORM craft_queue.admit(new_work_id, policy.repository, new_work_id,
        managed_order || jsonb_build_object('id', new_work_id), policy.token_limit);
    -- A cancelled work ID remains terminal and cannot be re-admitted by replay.
    IF EXISTS (SELECT 1 FROM craft_queue.standing_work AS binding WHERE binding.work_id = new_work_id) THEN
        RETURN jsonb_build_object('state', 'previously_admitted', 'work_id', new_work_id);
    END IF;
    INSERT INTO craft_queue.standing_work (
        work_id, rule_id, issue_key, source_revision, human_account, approval_reference, intent_approval_reference,
        preferred_runner, fallback_runner, fallback_at
    ) VALUES (
        new_work_id, policy.id, ticket, native_revision, policy.human_account, policy.approval_reference,
        CASE WHEN previous.source_revision = native_revision THEN previous.reassessment_reference END,
        policy.preferred_runner, policy.fallback_runner, now_at + make_interval(secs => policy.fallback_seconds)
    );
    RETURN jsonb_build_object('state', 'admitted', 'work_id', new_work_id);
END;
$$;

CREATE OR REPLACE FUNCTION craft_queue.claim(runner text, request uuid, context_capable boolean)
RETURNS TABLE (work_id text, repository text, generation bigint, expires_at timestamptz, work_order jsonb)
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    settings craft_queue.control%ROWTYPE;
    selected craft_queue.work%ROWTYPE;
    previous craft_queue.attempts%ROWTYPE;
    now_at timestamptz;
    review_free boolean;
    tokens_free numeric;
    deadline timestamptz;
BEGIN
    SELECT * INTO STRICT settings FROM craft_queue.control FOR UPDATE;
    now_at := clock_timestamp();
    SELECT * INTO previous FROM craft_queue.attempts AS a
    WHERE a.runner_id = runner AND a.request_id = request;
    IF FOUND THEN
        RETURN QUERY SELECT w.id, w.repository, w.generation, previous.expires_at,
            (w.payload - 'repository') || CASE WHEN w.repair_from_generation IS NULL
                THEN '{}'::jsonb ELSE '{"shared_repair":true}'::jsonb END
        FROM craft_queue.work AS w
        JOIN craft_queue.runner_repositories AS grant_row ON grant_row.repository = w.repository
        WHERE w.id = previous.work_id AND w.generation = previous.generation
          AND w.state = 'leased' AND previous.outcome = 'leased' AND previous.expires_at > now_at
          AND grant_row.runner_id = runner
          AND craft_queue.standing_runner_allowed(w.id, runner, now_at)
          AND (context_capable OR NOT EXISTS (SELECT 1 FROM craft_queue.project_context_pins(w.payload)));
        RETURN;
    END IF;
    IF settings.paused THEN RETURN; END IF;

    SELECT COUNT(*) < settings.review_limit INTO review_free
    FROM craft_queue.work AS w
    LEFT JOIN craft_queue.standing_work AS standing ON standing.work_id = w.id
    WHERE w.state IN ('leased', 'candidate')
      OR (w.state = 'queued' AND (w.repair_from_generation IS NOT NULL OR standing.work_id IS NOT NULL));
    SELECT settings.token_budget - COALESCE(SUM(COALESCE(a.reported_tokens, a.token_limit)), 0)
    INTO tokens_free FROM craft_queue.attempts AS a
    WHERE a.started_at > now_at - interval '24 hours'
       OR (a.outcome = 'leased' AND a.expires_at > now_at);

    SELECT w.* INTO selected
    FROM craft_queue.work AS w
    JOIN craft_queue.runner_repositories AS grant_row ON grant_row.repository = w.repository
    LEFT JOIN craft_queue.attempts AS a ON a.work_id = w.id AND a.generation = w.generation
    LEFT JOIN craft_queue.standing_work AS standing ON standing.work_id = w.id
    WHERE grant_row.runner_id = runner AND w.reserved_tokens <= tokens_free
      AND craft_queue.standing_runner_allowed(w.id, runner, now_at)
      AND (context_capable OR NOT EXISTS (SELECT 1 FROM craft_queue.project_context_pins(w.payload)))
      AND ((w.state = 'queued' AND (review_free OR w.repair_from_generation IS NOT NULL OR standing.work_id IS NOT NULL))
        OR (w.state = 'leased' AND a.outcome = 'leased' AND a.expires_at <= now_at))
    ORDER BY (w.state = 'leased' OR w.repair_from_generation IS NOT NULL OR standing.work_id IS NOT NULL) DESC,
        w.created_at, w.id
    LIMIT 1;
    IF NOT FOUND THEN RETURN; END IF;

    UPDATE craft_queue.attempts AS a SET outcome = 'expired'
    WHERE a.work_id = selected.id AND a.generation = selected.generation AND a.outcome = 'leased';
    UPDATE craft_queue.work AS w SET state = 'leased', generation = w.generation + 1
    WHERE w.id = selected.id RETURNING w.* INTO selected;
    deadline := now_at + make_interval(secs => settings.lease_seconds);
    INSERT INTO craft_queue.attempts (work_id, generation, runner_id, request_id, expires_at, started_at, token_limit)
    VALUES (selected.id, selected.generation, runner, request, deadline, now_at, selected.reserved_tokens);
    RETURN QUERY SELECT selected.id, selected.repository, selected.generation, deadline,
        (selected.payload - 'repository') || CASE WHEN selected.repair_from_generation IS NULL
            THEN '{}'::jsonb ELSE '{"shared_repair":true}'::jsonb END;
END;
$$;

ALTER FUNCTION craft_queue.status(text) RENAME TO status_before_standing;
CREATE FUNCTION craft_queue.status(work_id text) RETURNS jsonb
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT craft_queue.status_before_standing(work_id) || COALESCE((
        SELECT jsonb_build_object('standing_delegation', to_jsonb(binding) - 'work_id')
        FROM craft_queue.standing_work AS binding WHERE binding.work_id = status.work_id
    ), '{}'::jsonb);
$$;

ALTER FUNCTION craft_queue.operation_summary() RENAME TO operation_summary_before_standing;
CREATE FUNCTION craft_queue.operation_summary() RETURNS jsonb
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT sample.summary || jsonb_build_object(
        'queued_standing', standing.total,
        'review_reserved', (sample.summary->>'review_reserved')::bigint + standing.total
    )
    FROM (SELECT craft_queue.operation_summary_before_standing() AS summary) AS sample
    CROSS JOIN (
        SELECT count(*) AS total FROM craft_queue.work AS w
        JOIN craft_queue.standing_work AS binding ON binding.work_id = w.id
        WHERE w.state = 'queued' AND w.repair_from_generation IS NULL
    ) AS standing;
$$;

REVOKE ALL ON FUNCTION craft_queue.operation_summary() FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.status(text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.fence_standing_work(text, text, text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.disable_standing_rule(text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.standing_runner_allowed(text, text, timestamptz) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.approve_standing_intent(text, text, bigint, text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.observe_standing(text, text, jsonb, jsonb) FROM PUBLIC;
COMMIT;
