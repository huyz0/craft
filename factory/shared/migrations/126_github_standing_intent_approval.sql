BEGIN;

ALTER TABLE craft_queue.standing_observations
    ADD COLUMN github_pending_intent bytea,
    ADD COLUMN github_approved_intent bytea,
    ADD COLUMN github_approved_reference text;

ALTER FUNCTION craft_queue.observe_standing(text,text,jsonb,jsonb)
    RENAME TO observe_standing_before_github_approval;
ALTER FUNCTION craft_queue.observe_standing_before_github_approval(text,text,jsonb,jsonb) SECURITY INVOKER;

CREATE FUNCTION craft_queue.observe_standing(identity text, policy_id text, snapshot jsonb, candidate_order jsonb)
RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    ticket text := snapshot->>'issue';
    prior craft_queue.standing_observations%ROWTYPE;
    proposed_intent bytea;
    decision jsonb;
    github_rule boolean;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    SELECT EXISTS (SELECT 1 FROM craft_queue.standing_rules AS rule
        WHERE rule.id = policy_id AND rule.intake_id = identity AND rule.enabled
          AND rule.source = snapshot->>'source' AND rule.source LIKE 'github:%') INTO github_rule;
    IF github_rule AND jsonb_typeof(snapshot->'intent_approved') = 'boolean'
       AND jsonb_typeof(candidate_order) = 'object'
    THEN
        proposed_intent := sha256(convert_to(jsonb_build_array(snapshot->'source_issue_id',
            snapshot->'intent_digest', candidate_order - 'id' - 'repository' - 'owner' - 'issue')::text, 'UTF8'));
        SELECT * INTO prior FROM craft_queue.standing_observations AS observation
        WHERE observation.rule_id = policy_id AND observation.issue_key = ticket;
        -- The first sight of an unapproved input opens review. A subsequent relay
        -- confirmation can populate the exact review token but cannot clear review.
        IF proposed_intent IS DISTINCT FROM prior.github_approved_intent
           AND NOT COALESCE(prior.reassessment_required, false) THEN
            snapshot := jsonb_set(snapshot, '{intent_approved}', 'false');
        END IF;
    END IF;

    decision := craft_queue.observe_standing_before_github_approval(identity, policy_id, snapshot, candidate_order);
    IF github_rule AND decision->>'state' IN ('review_required', 'ineligible', 'invalid_order') THEN
        UPDATE craft_queue.standing_observations AS observation
        SET github_pending_intent = CASE WHEN decision->>'state' = 'review_required' THEN proposed_intent END
        WHERE observation.rule_id = policy_id AND observation.issue_key = ticket
          AND observation.source_revision = (snapshot->>'source_revision')::bigint;
    END IF;
    IF decision->>'state' = 'admitted' AND proposed_intent = prior.github_approved_intent THEN
        -- A timestamp-only change between review and admission must preserve the decision reference.
        UPDATE craft_queue.standing_work AS binding
        SET intent_approval_reference = prior.github_approved_reference
        WHERE binding.work_id = decision->>'work_id' AND binding.intent_approval_reference IS NULL;
    END IF;
    RETURN decision;
END;
$$;

CREATE OR REPLACE FUNCTION craft_queue.approve_managed_jira_intent(
    actor_id text, rule_key text, issue_key text, native_revision bigint, exact_token text, evidence_ref text
) RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    prior craft_queue.jira_standing_intent_approvals%ROWTYPE;
    rule_source text;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    SELECT approval.* INTO prior FROM craft_queue.jira_standing_intent_approvals AS approval
    WHERE approval.rule_id = rule_key AND approval.issue = issue_key
      AND approval.source_revision = native_revision AND approval.review_token = exact_token;
    IF FOUND THEN
        IF prior.actor = actor_id AND prior.evidence = evidence_ref THEN RETURN to_jsonb(prior); END IF;
        RETURN NULL;
    END IF;
    SELECT rule.source INTO rule_source FROM craft_queue.standing_rules AS rule
    WHERE rule.id = rule_key AND rule.enabled AND (rule.source LIKE 'jira:%' OR rule.source LIKE 'github:%');
    IF NOT FOUND OR EXISTS (SELECT 1 FROM craft_queue.recoveries WHERE finished_at IS NULL)
        OR EXISTS (SELECT 1 FROM craft_queue.standing_observations AS observation
            WHERE observation.rule_id = rule_key AND observation.issue_key = approve_managed_jira_intent.issue_key
              AND observation.github_conflict_revision >= native_revision)
        OR (rule_source LIKE 'github:%' AND NOT EXISTS (
            SELECT 1 FROM craft_queue.standing_observations AS observation
            WHERE observation.rule_id = rule_key AND observation.issue_key = approve_managed_jira_intent.issue_key
              AND observation.github_pending_intent IS NOT NULL))
        THEN RETURN NULL; END IF;
    IF NOT craft_queue.approve_jira_standing_intent(rule_key, issue_key, native_revision, exact_token, evidence_ref)
        THEN RETURN NULL; END IF;
    IF rule_source LIKE 'github:%' THEN
        UPDATE craft_queue.standing_observations AS observation
        SET github_approved_intent = observation.github_pending_intent,
            github_approved_reference = evidence_ref, github_pending_intent = NULL
        WHERE observation.rule_id = rule_key AND observation.issue_key = approve_managed_jira_intent.issue_key;
    END IF;
    INSERT INTO craft_queue.jira_standing_intent_approvals(rule_id, issue, source_revision, review_token, actor, evidence)
    VALUES (rule_key, issue_key, native_revision, exact_token, actor_id, evidence_ref)
    RETURNING * INTO prior;
    RETURN to_jsonb(prior);
END;
$$;

-- Version 125 did not retain a separately approved input digest. Reapproval
-- must start with a later source revision to avoid recreating a cancelled job.
DO $$
DECLARE
    prior record;
BEGIN
    FOR prior IN
        UPDATE craft_queue.standing_observations AS observation
        SET reassessment_required = true, reassessment_reference = NULL,
            github_conflict_revision = greatest(observation.github_conflict_revision, observation.source_revision)
        FROM craft_queue.standing_rules AS rule
        WHERE rule.id = observation.rule_id AND rule.source LIKE 'github:%'
          AND EXISTS (SELECT 1 FROM craft_queue.standing_work AS binding
              JOIN craft_queue.work AS work ON work.id = binding.work_id
              WHERE binding.rule_id = rule.id AND binding.issue_key = observation.issue_key
                AND work.state IN ('queued', 'leased', 'candidate') AND binding.blocked_at IS NULL)
        RETURNING observation.rule_id, observation.issue_key
    LOOP
        PERFORM craft_queue.fence_standing_work(prior.rule_id, prior.issue_key,
            'GitHub standing work needs a recorded operator intent approval');
    END LOOP;
END;
$$;

REVOKE ALL ON FUNCTION craft_queue.observe_standing_before_github_approval(text,text,jsonb,jsonb) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.observe_standing(text,text,jsonb,jsonb) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.approve_managed_jira_intent(text,text,text,bigint,text,text) FROM PUBLIC;

COMMIT;
