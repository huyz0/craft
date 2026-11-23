BEGIN;

CREATE TABLE craft_queue.knowledge_withdrawals (
    proposal_id text PRIMARY KEY REFERENCES craft_queue.knowledge_proposals(proposal_id),
    receipt jsonb NOT NULL
);

CREATE OR REPLACE FUNCTION craft_queue.knowledge_proposal(id text) RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT to_jsonb(p) || jsonb_build_object('withdrawal', w.receipt)
    FROM craft_queue.knowledge_proposals AS p
    LEFT JOIN craft_queue.knowledge_withdrawals AS w USING (proposal_id)
    WHERE p.proposal_id = id;
$$;

CREATE FUNCTION craft_queue.withdraw_knowledge(
    actor_id text, id text, expected_revision text, evidence_ref text
) RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    item craft_queue.knowledge_proposals%ROWTYPE;
    skill craft_queue.team_skill_releases%ROWTYPE;
    context craft_queue.project_context_releases%ROWTYPE;
    saved jsonb;
    destinations text[];
    retired_now text[] := ARRAY[]::text[];
    already_retired text[] := ARRAY[]::text[];
    target_location text;
    source_ref text;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    IF length(btrim(COALESCE(evidence_ref, ''))) NOT BETWEEN 1 AND 500
        OR evidence_ref ~ '[[:cntrl:]]' THEN RETURN NULL; END IF;
    SELECT p.* INTO item FROM craft_queue.knowledge_proposals AS p WHERE p.proposal_id = id;
    IF NOT FOUND OR item.decision->>'promoted' IS DISTINCT FROM 'true'
        OR item.decision->>'release_revision' IS DISTINCT FROM expected_revision THEN RETURN NULL; END IF;
    SELECT w.receipt INTO saved FROM craft_queue.knowledge_withdrawals AS w WHERE w.proposal_id = id;
    IF FOUND THEN
        IF saved->>'withdrawn_by' = actor_id AND saved->>'evidence' = evidence_ref THEN RETURN saved; END IF;
        RETURN NULL;
    END IF;

    target_location := item.proposal->>'location';
    destinations := CASE WHEN item.decision ? 'repositories' THEN
        ARRAY(SELECT jsonb_array_elements_text(item.decision->'repositories'))
        ELSE ARRAY[item.repository] END;
    IF item.proposal->>'kind' = 'skill' THEN
        SELECT s.* INTO skill FROM craft_queue.team_skill_releases AS s
        WHERE s.location = target_location AND s.revision = expected_revision
            AND s.owner_name = item.decision->>'owner' AND s.approval_reference = item.decision->>'evidence'
            AND s.repositories @> destinations AND s.repositories <@ destinations;
        IF NOT FOUND THEN RETURN NULL; END IF;
        IF skill.revoked_at IS NOT NULL THEN
            already_retired := destinations;
        ELSE
            IF craft_queue.withdraw_team_skill(actor_id, target_location, expected_revision, evidence_ref) IS NULL
                THEN RETURN NULL; END IF;
            retired_now := destinations;
        END IF;
    ELSE
        source_ref := 'craft:work/' || item.work_id || '/verification/' || (item.decision->>'verification_sequence');
        IF (SELECT count(*) FROM craft_queue.project_context_releases AS c
            WHERE c.location = target_location AND c.revision = expected_revision
                AND c.repository = ANY(destinations) AND c.source_reference = source_ref
                AND c.source_revision = item.head AND c.approval_reference = item.decision->>'evidence'
        ) <> cardinality(destinations) THEN RETURN NULL; END IF;
        BEGIN
            FOR context IN SELECT c.* FROM craft_queue.project_context_releases AS c
                WHERE c.location = target_location AND c.revision = expected_revision
                  AND c.repository = ANY(destinations) ORDER BY c.repository
            LOOP
                IF context.retired_at IS NOT NULL THEN
                    already_retired := array_append(already_retired, context.repository);
                ELSE
                    IF craft_queue.withdraw_project_context(
                        actor_id, context.repository, target_location, expected_revision, evidence_ref
                    ) IS NULL THEN RAISE SQLSTATE 'P1090'; END IF;
                    retired_now := array_append(retired_now, context.repository);
                END IF;
            END LOOP;
        EXCEPTION WHEN SQLSTATE 'P1090' THEN
            RETURN NULL;
        END;
    END IF;

    saved := jsonb_build_object('proposal_id', id, 'release_revision', expected_revision,
        'location', target_location, 'kind', item.proposal->>'kind', 'withdrawn_by', actor_id,
        'evidence', evidence_ref, 'withdrawn_at', clock_timestamp(),
        'retired_now', retired_now, 'already_retired', already_retired);
    INSERT INTO craft_queue.knowledge_withdrawals (proposal_id, receipt) VALUES (id, saved);
    RETURN saved;
END;
$$;

REVOKE ALL ON FUNCTION craft_queue.withdraw_knowledge(text,text,text,text) FROM PUBLIC;

COMMIT;
