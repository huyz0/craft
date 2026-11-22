BEGIN;

CREATE OR REPLACE FUNCTION craft_queue.approve_jira_standing_intent(
    policy_id text, ticket text, reviewed_revision bigint, review_token text, approval_ref text
) RETURNS boolean
LANGUAGE plpgsql SET search_path = pg_catalog AS $$
DECLARE
    oldest_read bigint;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    oldest_read := floor(extract(epoch FROM clock_timestamp()))::bigint - 300;
    UPDATE craft_queue.standing_observations AS observation
    SET reassessment_required = false, reassessment_reference = approval_ref
    FROM craft_queue.standing_rules AS rule
    WHERE observation.rule_id = policy_id AND observation.issue_key = ticket
      AND observation.source_revision = reviewed_revision AND observation.reassessment_required
      AND observation.jira_source_digest IS NOT NULL AND observation.jira_source_approved
      AND observation.source_read_at >= oldest_read
      AND rule.id = observation.rule_id
      AND (rule.source NOT LIKE 'jira:%' OR observation.jira_review_fingerprint = observation.fingerprint)
      AND review_token = 'sha256:' || encode(observation.fingerprint, 'hex')
      AND length(btrim(approval_ref)) BETWEEN 1 AND 500;
    RETURN FOUND;
END;
$$;

COMMIT;
