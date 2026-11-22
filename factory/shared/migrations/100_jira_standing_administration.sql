BEGIN;

ALTER TABLE craft_queue.standing_rules
    ADD COLUMN approved_by text,
    ADD COLUMN approved_at timestamptz,
    ADD COLUMN policy_digest text,
    ADD COLUMN disabled_by text,
    ADD COLUMN disabled_at timestamptz,
    ADD COLUMN disable_evidence text;

CREATE TABLE craft_queue.jira_standing_intent_approvals (
    rule_id text NOT NULL REFERENCES craft_queue.standing_rules(id),
    issue text NOT NULL,
    source_revision bigint NOT NULL,
    review_token text NOT NULL,
    actor text NOT NULL,
    evidence text NOT NULL,
    approved_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (rule_id, issue, source_revision, review_token)
);

CREATE FUNCTION craft_queue.register_jira_standing_rule(actor_id text, policy_pin text, policy jsonb)
RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    desired craft_queue.standing_rules%ROWTYPE;
    prior craft_queue.standing_rules%ROWTYPE;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    desired := jsonb_populate_record(NULL::craft_queue.standing_rules, policy);
    desired.enabled := true;
    desired.approved_by := actor_id;
    desired.policy_digest := policy_pin;
    SELECT r.* INTO prior FROM craft_queue.standing_rules AS r WHERE r.id = desired.id;
    IF FOUND THEN
        IF prior.enabled AND to_jsonb(prior) - 'approved_at' = to_jsonb(desired) - 'approved_at'
            THEN RETURN to_jsonb(prior); END IF;
        RETURN NULL;
    END IF;
    IF EXISTS (SELECT 1 FROM craft_queue.recoveries WHERE finished_at IS NULL)
        OR NOT EXISTS (SELECT 1 FROM craft_queue.runner_repositories AS rr
            WHERE rr.repository = desired.repository AND rr.runner_id = desired.preferred_runner)
        OR (desired.fallback_runner IS NOT NULL AND NOT EXISTS (
            SELECT 1 FROM craft_queue.runner_repositories AS rr
            WHERE rr.repository = desired.repository AND rr.runner_id = desired.fallback_runner))
        THEN RETURN NULL; END IF;
    desired.approved_at := clock_timestamp();
    INSERT INTO craft_queue.standing_rules SELECT (desired).* RETURNING * INTO prior;
    RETURN to_jsonb(prior);
END;
$$;

CREATE FUNCTION craft_queue.disable_managed_standing_rule(actor_id text, rule_key text, evidence_ref text)
RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    prior craft_queue.standing_rules%ROWTYPE;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    SELECT r.* INTO prior FROM craft_queue.standing_rules AS r WHERE r.id = rule_key AND r.source LIKE 'jira:%';
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

CREATE FUNCTION craft_queue.approve_managed_jira_intent(
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
            WHERE r.id = rule_key AND r.enabled AND r.source LIKE 'jira:%')
        OR NOT craft_queue.approve_jira_standing_intent(rule_key, issue_key, native_revision, exact_token, evidence_ref)
        THEN RETURN NULL; END IF;
    INSERT INTO craft_queue.jira_standing_intent_approvals(rule_id, issue, source_revision, review_token, actor, evidence)
    VALUES (rule_key, issue_key, native_revision, exact_token, actor_id, evidence_ref)
    RETURNING * INTO prior;
    RETURN to_jsonb(prior);
END;
$$;

REVOKE ALL ON FUNCTION craft_queue.register_jira_standing_rule(text,text,jsonb) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.disable_managed_standing_rule(text,text,text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.approve_managed_jira_intent(text,text,text,bigint,text,text) FROM PUBLIC;

COMMIT;
