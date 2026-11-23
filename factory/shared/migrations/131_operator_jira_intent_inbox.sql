BEGIN;

ALTER TABLE craft_queue.standing_observations
    ADD COLUMN jira_review_fingerprint bytea;

CREATE INDEX standing_observations_pending_jira_review
    ON craft_queue.standing_observations (rule_id, source_read_at, issue_key)
    WHERE reassessment_required AND jira_review_fingerprint IS NOT NULL;

ALTER FUNCTION craft_queue.observe_standing(text,text,jsonb,jsonb)
    RENAME TO observe_standing_before_jira_inbox;
ALTER FUNCTION craft_queue.observe_standing_before_jira_inbox(text,text,jsonb,jsonb) SECURITY INVOKER;

CREATE FUNCTION craft_queue.observe_standing(identity text, policy_id text, snapshot jsonb, candidate_order jsonb)
RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    decision jsonb;
BEGIN
    decision := craft_queue.observe_standing_before_jira_inbox(identity, policy_id, snapshot, candidate_order);
    -- Rejected envelopes leave the last accepted Jira decision unchanged.
    IF snapshot->>'source' LIKE 'jira:%'
       AND decision->>'state' IN ('review_required', 'ineligible', 'admitted', 'already_admitted',
                              'existing_work', 'scope_not_allowed') THEN
        UPDATE craft_queue.standing_observations AS observation
        SET jira_review_fingerprint = CASE WHEN decision->>'state' = 'review_required' THEN observation.fingerprint END
        FROM craft_queue.standing_rules AS rule
        WHERE rule.id = policy_id AND rule.intake_id = identity
          AND rule.source = snapshot->>'source'
          AND rule.source ~ '^jira:[a-f0-9]{8}(-[a-f0-9]{4}){3}-[a-f0-9]{12}$'
          AND observation.rule_id = rule.id AND observation.issue_key = snapshot->>'issue'
          AND observation.source_revision = (snapshot->>'source_revision')::bigint
          AND observation.jira_review_fingerprint IS DISTINCT FROM
              (CASE WHEN decision->>'state' = 'review_required' THEN observation.fingerprint END);
    END IF;
    RETURN decision;
END;
$$;

ALTER FUNCTION craft_queue.operator_brief(text,integer,integer)
    RENAME TO operator_brief_before_jira_intents;
ALTER FUNCTION craft_queue.operator_brief_before_jira_intents(text,integer,integer) SECURITY INVOKER;

CREATE FUNCTION craft_queue.operator_brief(repo_filter text, usage_hours integer, item_limit integer)
RETURNS jsonb LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    WITH brief AS MATERIALIZED (
        SELECT craft_queue.operator_brief_before_jira_intents(repo_filter, usage_hours, item_limit) AS report
    ), jira AS MATERIALIZED (
        SELECT rule.id AS rule_id, rule.repository, rule.human_owner AS owner,
            substring(rule.source FROM 6) AS cloud_id,
            observation.issue_key AS issue, observation.source_revision,
            to_timestamp(observation.source_read_at) AS observed_at, observation.fingerprint,
            CASE
                WHEN observation.source_read_at < floor(extract(epoch FROM statement_timestamp())) - 300
                    THEN 'refresh_observation'
                WHEN NOT observation.jira_source_approved THEN 'confirm_relay_input'
                ELSE 'ready_for_approval'
            END AS state
        FROM craft_queue.standing_observations AS observation
        JOIN craft_queue.standing_rules AS rule ON rule.id = observation.rule_id
        WHERE rule.enabled AND rule.source ~ '^jira:[a-f0-9]{8}(-[a-f0-9]{4}){3}-[a-f0-9]{12}$'
          AND (repo_filter IS NULL OR rule.repository = repo_filter)
          AND observation.reassessment_required AND observation.jira_review_fingerprint IS NOT NULL
          AND observation.jira_review_fingerprint = observation.fingerprint
    ), items AS (
        SELECT jsonb_strip_nulls(jsonb_build_object(
            'tracker', 'jira', 'rule_id', rule_id, 'repository', repository, 'owner', owner,
            'cloud_id', cloud_id, 'issue', issue, 'source_revision', source_revision,
            'observed_at', observed_at, 'state', state,
            'review_token', CASE WHEN state = 'ready_for_approval' THEN 'sha256:' || encode(fingerprint, 'hex') END
        )) AS item FROM jira
        UNION ALL
        SELECT entry.item || jsonb_build_object('tracker', 'github')
        FROM brief CROSS JOIN LATERAL jsonb_array_elements(report#>'{standing_intent_reviews,items}') AS entry(item)
    ), page AS (
        SELECT item, row_number() OVER (
            ORDER BY (item->>'state' = 'ready_for_approval') DESC, (item->>'observed_at')::timestamptz,
                (item->>'rule_id') COLLATE "C", (item->>'issue') COLLATE "C"
        ) AS position FROM items
    )
    SELECT jsonb_set(report, '{standing_intent_reviews}', jsonb_build_object(
        'total', (report#>>'{standing_intent_reviews,total}')::bigint + (SELECT count(*) FROM jira),
        'ready', (report#>>'{standing_intent_reviews,ready}')::bigint
            + (SELECT count(*) FROM jira WHERE state = 'ready_for_approval'),
        'items', COALESCE((SELECT jsonb_agg(item ORDER BY position) FROM page
            WHERE position <= greatest(0, least(item_limit, 20))), '[]'::jsonb)
    )) FROM brief;
$$;

REVOKE ALL ON FUNCTION craft_queue.observe_standing_before_jira_inbox(text,text,jsonb,jsonb) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.observe_standing(text,text,jsonb,jsonb) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.operator_brief_before_jira_intents(text,integer,integer) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.operator_brief(text,integer,integer) FROM PUBLIC;

COMMIT;
