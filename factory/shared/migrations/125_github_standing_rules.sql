BEGIN;

ALTER TABLE craft_queue.standing_observations ADD COLUMN github_conflict_revision bigint;

ALTER TABLE craft_queue.standing_rules DROP CONSTRAINT standing_rule_execution_profile;
ALTER TABLE craft_queue.standing_rules ADD CONSTRAINT standing_rule_execution_profile CHECK (
    (execution_profile IS NULL AND agent_adapter IS NULL) OR COALESCE(
        (source LIKE 'jira:%' OR source LIKE 'github:%') AND approved_by IS NOT NULL
        AND jsonb_typeof(execution_profile) = 'object'
        AND craft_queue.execution_profile_valid(execution_profile)
        AND COALESCE(agent_adapter, 'codex') IN ('codex', 'claude')
        AND craft_queue.execution_effort_supported(agent_adapter, execution_profile->>'reasoning_effort'),
        false)
);

CREATE OR REPLACE FUNCTION craft_queue.observe_standing(identity text, policy_id text, snapshot jsonb, candidate_order jsonb)
RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    policy_source text;
    intent_digest text := snapshot->>'intent_digest';
    native_id text := snapshot->>'source_issue_id';
    native_revision bigint := (snapshot->>'source_revision')::bigint;
    expected_pin text;
    pin_prefix text;
    context_pins jsonb;
    matching_pins bigint;
    tracker_pins bigint;
    decision jsonb;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    SELECT rule.source INTO policy_source FROM craft_queue.standing_rules AS rule
    WHERE rule.id = policy_id AND rule.intake_id = identity AND rule.enabled AND rule.source = snapshot->>'source'
      AND (rule.source ~ '^jira:[a-f0-9]{8}(-[a-f0-9]{4}){3}-[a-f0-9]{12}$'
           OR rule.source LIKE 'github:%');
    IF FOUND THEN
        IF COALESCE(intent_digest, '') !~ '^sha256:[a-f0-9]{64}$'
           OR jsonb_typeof(snapshot->'intent_digest') IS DISTINCT FROM 'string'
           OR jsonb_typeof(snapshot->'intent_approved') IS DISTINCT FROM 'boolean' THEN
            RETURN jsonb_build_object('state', 'invalid_observation');
        END IF;
        IF policy_source LIKE 'github:%' THEN
            IF jsonb_typeof(snapshot->'source_issue_id') IS DISTINCT FROM 'number'
               OR COALESCE(native_id, '') !~ '^[1-9][0-9]{0,19}$' THEN
                RETURN jsonb_build_object('state', 'invalid_observation');
            END IF;
            pin_prefix := 'github-intent:';
            expected_pin := pin_prefix || substring(policy_source FROM 8) || '/' || native_id;
            IF EXISTS (
                SELECT 1 FROM craft_queue.standing_observations AS prior
                WHERE prior.rule_id = policy_id AND prior.issue_key = snapshot->>'issue'
                  AND prior.github_conflict_revision >= native_revision
            ) THEN
                RETURN jsonb_build_object('state', 'stale_or_conflicting');
            END IF;
        ELSE
            pin_prefix := 'jira-intent:';
        END IF;
        IF jsonb_typeof(candidate_order) = 'object' THEN
            context_pins := candidate_order->'context';
            IF jsonb_typeof(context_pins) IS DISTINCT FROM 'array' THEN
                RETURN jsonb_build_object('state', 'invalid_order');
            END IF;
            SELECT count(*), count(*) FILTER (
                WHERE pin->>'revision' = intent_digest AND (
                    (expected_pin IS NOT NULL AND pin->>'location' = expected_pin)
                    OR (expected_pin IS NULL AND pin->>'location' ~ (
                        '^jira-intent:' || substring(policy_source FROM 6) || '/[1-9][0-9]{0,18}$')))
            ) INTO tracker_pins, matching_pins
            FROM jsonb_array_elements(context_pins) AS context(pin)
            WHERE starts_with(pin->>'location', pin_prefix);
            IF tracker_pins <> 1 OR matching_pins <> 1 THEN
                RETURN jsonb_build_object('state', 'invalid_order');
            END IF;
        END IF;
    END IF;
    decision := craft_queue.observe_standing_before_intent_guard(identity, policy_id, snapshot, candidate_order);
    -- The GitHub clock has second precision. Conflicting observations at the same
    -- revision must stop work without replacing the most recent valid record.
    IF policy_source LIKE 'github:%' AND decision->>'state' = 'stale_or_conflicting' THEN
        UPDATE craft_queue.standing_observations AS prior
        SET reassessment_required = true, reassessment_reference = NULL,
            github_conflict_revision = native_revision
        WHERE prior.rule_id = policy_id AND prior.issue_key = snapshot->>'issue'
          AND prior.source_revision = native_revision
          AND prior.jira_source_digest IS DISTINCT FROM
              sha256(convert_to((snapshot - 'observed_at' - 'intent_approved')::text, 'UTF8'));
        IF FOUND THEN
            PERFORM craft_queue.fence_standing_work(policy_id, snapshot->>'issue',
                'GitHub issue has conflicting observations at the same revision');
        END IF;
    END IF;
    RETURN decision;
END;
$$;

-- Earlier generic relays could use a GitHub source name before native identity was required.
DO $$
DECLARE
    prior record;
BEGIN
    FOR prior IN
        UPDATE craft_queue.standing_observations AS observation
        SET reassessment_required = true, reassessment_reference = NULL,
            github_conflict_revision = observation.source_revision
        FROM craft_queue.standing_rules AS rule
        WHERE rule.id = observation.rule_id AND rule.source LIKE 'github:%'
        RETURNING observation.rule_id, observation.issue_key
    LOOP
        PERFORM craft_queue.fence_standing_work(prior.rule_id, prior.issue_key,
            'Standing GitHub issue needs a native identity pin');
    END LOOP;
END;
$$;

CREATE OR REPLACE FUNCTION craft_queue.disable_managed_standing_rule(actor_id text, rule_key text, evidence_ref text)
RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    prior craft_queue.standing_rules%ROWTYPE;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    SELECT r.* INTO prior FROM craft_queue.standing_rules AS r WHERE r.id = rule_key AND (r.source LIKE 'jira:%' OR r.source LIKE 'github:%');
    IF NOT FOUND THEN RETURN NULL; END IF;
    IF NOT prior.enabled THEN
        IF prior.disabled_by = actor_id AND prior.disable_evidence = evidence_ref THEN RETURN to_jsonb(prior); END IF;
        RETURN NULL;
    END IF;
    PERFORM craft_queue.disable_standing_rule(rule_key);
    UPDATE craft_queue.standing_rules AS r SET disabled_by = actor_id, disabled_at = clock_timestamp(),
        disable_evidence = evidence_ref WHERE r.id = rule_key RETURNING * INTO prior;
    RETURN to_jsonb(prior);
END;
$$;

CREATE OR REPLACE FUNCTION craft_queue.approve_managed_jira_intent(
    actor_id text, rule_key text, issue_key text, native_revision bigint, exact_token text, evidence_ref text
) RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    prior craft_queue.jira_standing_intent_approvals%ROWTYPE;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    SELECT a.* INTO prior FROM craft_queue.jira_standing_intent_approvals AS a
    WHERE a.rule_id = rule_key AND a.issue = issue_key AND a.source_revision = native_revision
      AND a.review_token = exact_token;
    IF FOUND THEN
        IF prior.actor = actor_id AND prior.evidence = evidence_ref THEN RETURN to_jsonb(prior); END IF;
        RETURN NULL;
    END IF;
    IF EXISTS (SELECT 1 FROM craft_queue.recoveries WHERE finished_at IS NULL)
        OR NOT EXISTS (SELECT 1 FROM craft_queue.standing_rules AS r
            WHERE r.id = rule_key AND r.enabled AND (r.source LIKE 'jira:%' OR r.source LIKE 'github:%'))
        OR EXISTS (SELECT 1 FROM craft_queue.standing_observations AS observation
            WHERE observation.rule_id = rule_key AND observation.issue_key = approve_managed_jira_intent.issue_key
              AND observation.github_conflict_revision >= native_revision)
        OR NOT craft_queue.approve_jira_standing_intent(rule_key, issue_key, native_revision, exact_token, evidence_ref)
        THEN RETURN NULL; END IF;
    INSERT INTO craft_queue.jira_standing_intent_approvals(rule_id, issue, source_revision, review_token, actor, evidence)
    VALUES (rule_key, issue_key, native_revision, exact_token, actor_id, evidence_ref)
    RETURNING * INTO prior;
    RETURN to_jsonb(prior);
END;
$$;

COMMIT;
