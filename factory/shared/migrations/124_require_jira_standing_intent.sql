BEGIN;

ALTER FUNCTION craft_queue.observe_standing(text,text,jsonb,jsonb)
    RENAME TO observe_standing_before_intent_guard;
ALTER FUNCTION craft_queue.observe_standing_before_intent_guard(text,text,jsonb,jsonb) SECURITY INVOKER;

CREATE FUNCTION craft_queue.observe_standing(identity text, policy_id text, snapshot jsonb, candidate_order jsonb)
RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    jira_digest text := snapshot->>'intent_digest';
    jira_site text;
    context_pins jsonb;
    matching_pins bigint;
    jira_pins bigint;
BEGIN
    -- Keep policy registration and the protected observation in the same order as admission.
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    SELECT substring(rule.source FROM 6) INTO jira_site
    FROM craft_queue.standing_rules AS rule
    WHERE rule.id = policy_id AND rule.intake_id = identity AND rule.enabled
      AND rule.source = snapshot->>'source'
      AND rule.source ~ '^jira:[a-f0-9]{8}(-[a-f0-9]{4}){3}-[a-f0-9]{12}$';
    IF FOUND THEN
        IF COALESCE(jira_digest, '') !~ '^sha256:[a-f0-9]{64}$'
           OR jsonb_typeof(snapshot->'intent_digest') IS DISTINCT FROM 'string'
           OR jsonb_typeof(snapshot->'intent_approved') IS DISTINCT FROM 'boolean' THEN
            RETURN jsonb_build_object('state', 'invalid_observation');
        END IF;
        IF jsonb_typeof(candidate_order) = 'object' THEN
            context_pins := candidate_order->'context';
            IF jsonb_typeof(context_pins) IS DISTINCT FROM 'array' THEN
                RETURN jsonb_build_object('state', 'invalid_order');
            END IF;
            SELECT count(*), count(*) FILTER (
                WHERE pin->>'revision' = jira_digest
                  AND pin->>'location' ~ ('^jira-intent:' || jira_site || '/[1-9][0-9]{0,18}$')
            ) INTO jira_pins, matching_pins
            FROM jsonb_array_elements(context_pins) AS context(pin)
            WHERE pin->>'location' LIKE 'jira-intent:%';
            IF jira_pins <> 1 OR matching_pins <> 1 THEN
                RETURN jsonb_build_object('state', 'invalid_order');
            END IF;
        END IF;
    END IF;
    RETURN craft_queue.observe_standing_before_intent_guard(identity, policy_id, snapshot, candidate_order);
END;
$$;

-- Preserve the work and observation records. Require an explicit review of a later Jira revision.
DO $$
DECLARE
    unpinned record;
BEGIN
    FOR unpinned IN
        UPDATE craft_queue.standing_observations AS observation
        SET reassessment_required = true, reassessment_reference = NULL
        FROM craft_queue.standing_rules AS rule
        WHERE rule.id = observation.rule_id AND observation.jira_source_digest IS NULL
          AND rule.source ~ '^jira:[a-f0-9]{8}(-[a-f0-9]{4}){3}-[a-f0-9]{12}$'
        RETURNING observation.rule_id, observation.issue_key
    LOOP
        PERFORM craft_queue.fence_standing_work(unpinned.rule_id, unpinned.issue_key,
            'Jira standing observation lacks an intent digest');
    END LOOP;
END;
$$;

REVOKE ALL ON FUNCTION craft_queue.observe_standing_before_intent_guard(text,text,jsonb,jsonb) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.observe_standing(text,text,jsonb,jsonb) FROM PUBLIC;

COMMIT;
