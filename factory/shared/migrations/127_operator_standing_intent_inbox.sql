BEGIN;

CREATE INDEX standing_observations_pending_github_review
    ON craft_queue.standing_observations (rule_id, source_read_at, issue_key)
    WHERE reassessment_required AND github_pending_intent IS NOT NULL;

ALTER FUNCTION craft_queue.operator_brief(text,integer,integer)
    RENAME TO operator_brief_before_standing_intents;
ALTER FUNCTION craft_queue.operator_brief_before_standing_intents(text,integer,integer) SECURITY INVOKER;

CREATE FUNCTION craft_queue.operator_brief(repo_filter text, usage_hours integer, item_limit integer)
RETURNS jsonb LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    WITH pending AS MATERIALIZED (
        SELECT rule.id AS rule_id, rule.repository, rule.human_owner AS owner, rule.source,
            observation.issue_key AS issue, observation.source_revision,
            observation.source_read_at, observation.fingerprint,
            CASE
                WHEN observation.source_read_at < floor(extract(epoch FROM statement_timestamp())) - 300
                    THEN 'refresh_observation'
                WHEN NOT observation.jira_source_approved THEN 'confirm_relay_input'
                ELSE 'ready_for_approval'
            END AS state
        FROM craft_queue.standing_observations AS observation
        JOIN craft_queue.standing_rules AS rule ON rule.id = observation.rule_id
        WHERE rule.enabled AND rule.source LIKE 'github:%'
          AND observation.issue_key ~ '^GITHUB-[1-9][0-9]{0,17}$'
          AND (repo_filter IS NULL OR rule.repository = repo_filter)
          AND observation.reassessment_required AND observation.github_pending_intent IS NOT NULL
          AND (observation.github_conflict_revision IS NULL
               OR observation.github_conflict_revision < observation.source_revision)
    ), page AS (
        SELECT * FROM pending
        ORDER BY (state = 'ready_for_approval') DESC, source_read_at, rule_id COLLATE "C", issue COLLATE "C"
        LIMIT greatest(0, least(item_limit, 20))
    )
    SELECT craft_queue.operator_brief_before_standing_intents(repo_filter, usage_hours, item_limit)
        || jsonb_build_object('standing_intent_reviews', jsonb_build_object(
            'total', (SELECT count(*) FROM pending),
            'ready', (SELECT count(*) FROM pending WHERE state = 'ready_for_approval'),
            'items', COALESCE((
                SELECT jsonb_agg(jsonb_strip_nulls(jsonb_build_object(
                    'rule_id', rule_id, 'repository', repository, 'owner', owner, 'issue', issue,
                    'issue_url', 'https://github.com/' || substring(source FROM 8)
                        || '/issues/' || substring(issue FROM 8),
                    'source_revision', source_revision, 'observed_at', to_timestamp(source_read_at),
                    'state', state,
                    'review_token', CASE WHEN state = 'ready_for_approval' THEN 'sha256:' || encode(fingerprint, 'hex') END
                )) ORDER BY (state = 'ready_for_approval') DESC, source_read_at,
                    rule_id COLLATE "C", issue COLLATE "C")
                FROM page
            ), '[]'::jsonb)
        ));
$$;

REVOKE ALL ON FUNCTION craft_queue.operator_brief_before_standing_intents(text,integer,integer) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.operator_brief(text,integer,integer) FROM PUBLIC;

COMMIT;
