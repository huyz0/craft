BEGIN;

ALTER TABLE craft_queue.standing_observations
    ADD COLUMN jira_source_digest bytea,
    ADD COLUMN jira_source_approved boolean NOT NULL DEFAULT false;

CREATE OR REPLACE FUNCTION craft_queue.approve_standing_intent(
    policy_id text, ticket text, reviewed_revision bigint, approval_ref text
) RETURNS boolean
LANGUAGE plpgsql SET search_path = pg_catalog AS $$
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    UPDATE craft_queue.standing_observations AS observation
    SET reassessment_required = false, reassessment_reference = approval_ref
    WHERE observation.rule_id = policy_id AND observation.issue_key = ticket
      AND observation.source_revision = reviewed_revision AND observation.reassessment_required
      AND observation.jira_source_digest IS NULL AND length(btrim(approval_ref)) BETWEEN 1 AND 500;
    RETURN FOUND;
END;
$$;

CREATE FUNCTION craft_queue.approve_jira_standing_intent(
    policy_id text, ticket text, reviewed_revision bigint, review_token text, approval_ref text
) RETURNS boolean
LANGUAGE plpgsql SET search_path = pg_catalog AS $$
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    UPDATE craft_queue.standing_observations AS observation
    SET reassessment_required = false, reassessment_reference = approval_ref
    WHERE observation.rule_id = policy_id AND observation.issue_key = ticket
      AND observation.source_revision = reviewed_revision AND observation.reassessment_required
      AND observation.jira_source_digest IS NOT NULL AND observation.jira_source_approved
      AND review_token = 'sha256:' || encode(observation.fingerprint, 'hex')
      AND length(btrim(approval_ref)) BETWEEN 1 AND 500;
    RETURN FOUND;
END;
$$;

REVOKE ALL ON FUNCTION craft_queue.approve_jira_standing_intent(text, text, bigint, text, text) FROM PUBLIC;

CREATE OR REPLACE FUNCTION craft_queue.observe_standing(
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
    jira_source_digest bytea;
    source_details jsonb := snapshot - 'observed_at' - 'intent_approved';
    approved_source boolean := COALESCE((snapshot->>'intent_approved')::boolean, false);
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
    IF snapshot ? 'intent_digest' THEN
        IF snapshot->>'intent_digest' !~ '^sha256:[a-f0-9]{64}$'
            OR jsonb_typeof(snapshot->'intent_approved') IS DISTINCT FROM 'boolean' THEN
            RETURN jsonb_build_object('state', 'invalid_observation');
        END IF;
        jira_source_digest := sha256(convert_to(source_details::text, 'UTF8'));
    END IF;
    observation_digest := sha256(convert_to(
        jsonb_build_array(source_details, candidate_order)::text, 'UTF8'));
    SELECT * INTO previous FROM craft_queue.standing_observations AS observation
    WHERE observation.rule_id = policy_id AND observation.issue_key = ticket;
    IF previous.jira_source_digest IS NOT NULL AND jira_source_digest IS NULL THEN
        RETURN jsonb_build_object('state', 'invalid_observation');
    END IF;
    IF previous.source_revision IS NOT NULL AND (
        native_revision < previous.source_revision OR (
            native_revision = previous.source_revision AND observation_digest <> previous.fingerprint
            AND NOT (previous.reassessment_required AND jira_source_digest IS NOT NULL
                AND jira_source_digest IS NOT DISTINCT FROM previous.jira_source_digest)
        )
    ) THEN RETURN jsonb_build_object('state', 'stale_or_conflicting'); END IF;
    INSERT INTO craft_queue.standing_observations (
        rule_id, issue_key, source_revision, source_read_at, fingerprint, jira_source_digest, jira_source_approved
    ) VALUES (policy_id, ticket, native_revision, read_at, observation_digest, jira_source_digest, approved_source)
    ON CONFLICT (rule_id, issue_key) DO UPDATE
        SET source_revision = EXCLUDED.source_revision,
            source_read_at = greatest(standing_observations.source_read_at, EXCLUDED.source_read_at),
            fingerprint = EXCLUDED.fingerprint, jira_source_digest = EXCLUDED.jira_source_digest,
            jira_source_approved = EXCLUDED.jira_source_approved,
            reassessment_reference = CASE WHEN standing_observations.source_revision = EXCLUDED.source_revision
                THEN standing_observations.reassessment_reference ELSE NULL END;

    IF snapshot->>'assignee' IS DISTINCT FROM policy.human_account
       OR snapshot->>'status' IS DISTINCT FROM policy.eligible_status
       OR NOT (snapshot->'labels' ? policy.required_label) THEN
        PERFORM craft_queue.fence_standing_work(policy_id, ticket, 'Standing delegation no longer matches the ticket');
        RETURN jsonb_build_object('state', 'ineligible');
    END IF;
    IF jira_source_digest IS NOT NULL AND NOT approved_source THEN
        PERFORM craft_queue.fence_standing_work(policy_id, ticket, 'Jira intent differs from the approved digest');
        UPDATE craft_queue.standing_observations AS observation
        SET reassessment_required = true, reassessment_reference = NULL
        WHERE observation.rule_id = policy_id AND observation.issue_key = ticket;
        RETURN jsonb_build_object('state', 'review_required',
            'review_token', 'sha256:' || encode(observation_digest, 'hex'));
    END IF;
    IF previous.reassessment_required THEN
        RETURN jsonb_build_object('state', 'review_required') || CASE WHEN jira_source_digest IS NULL
            THEN '{}'::jsonb ELSE jsonb_build_object('review_token', 'sha256:' || encode(observation_digest, 'hex')) END;
    END IF;
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
        RETURN jsonb_build_object('state', 'review_required') || CASE WHEN jira_source_digest IS NULL
            THEN '{}'::jsonb ELSE jsonb_build_object('review_token', 'sha256:' || encode(observation_digest, 'hex')) END;
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

COMMIT;
