BEGIN;

CREATE FUNCTION craft_queue.review_knowledge(
    actor text, id text, expected_digest text, promote boolean, source_owner text, evidence text, expected_verdict bigint,
    approved_repositories text[]
) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    item craft_queue.knowledge_proposals%ROWTYPE;
    verdict craft_queue.verifications%ROWTYPE;
    result jsonb;
    revision text;
    kind text;
    area text;
    body text;
    source_ref text;
    destinations text[];
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    IF promote IS NULL OR length(btrim(COALESCE(source_owner, ''))) NOT BETWEEN 1 AND 200
        OR length(btrim(COALESCE(evidence, ''))) NOT BETWEEN 1 AND 500
        OR source_owner ~ '[[:cntrl:]]' OR evidence ~ '[[:cntrl:]]' THEN RETURN NULL; END IF;
    SELECT * INTO item FROM craft_queue.knowledge_proposals AS p WHERE p.proposal_id = id;
    IF NOT FOUND OR item.proposal_digest IS DISTINCT FROM expected_digest OR item.proposed_by = actor THEN
        RETURN NULL;
    END IF;
    IF approved_repositories IS NOT NULL THEN
        IF NOT promote OR cardinality(approved_repositories) NOT BETWEEN 1 AND 16
            OR array_position(approved_repositories, NULL) IS NOT NULL
            OR NOT item.repository = ANY (approved_repositories)
            OR EXISTS (SELECT 1 FROM unnest(approved_repositories) AS target(repo)
                       WHERE length(repo) > 200
                           OR repo !~ '^[A-Za-z0-9][A-Za-z0-9._-]{0,99}/[A-Za-z0-9][A-Za-z0-9._-]{0,99}$')
            THEN RETURN NULL; END IF;
        SELECT array_agg(repo ORDER BY repo) INTO destinations FROM (
            SELECT DISTINCT repo FROM unnest(approved_repositories) AS target(repo)
        ) AS unique_targets;
        IF cardinality(destinations) <> cardinality(approved_repositories) THEN RETURN NULL; END IF;
    ELSE
        destinations := ARRAY[item.repository];
    END IF;
    result := jsonb_build_object('promoted', promote, 'recorded_by', actor, 'owner', source_owner,
        'evidence', evidence, 'verification_sequence', expected_verdict);
    IF approved_repositories IS NOT NULL THEN
        result := result || jsonb_build_object('repositories', destinations);
    END IF;
    IF item.decision IS NOT NULL THEN
        IF item.decision - 'decided_at' - 'release_revision' = result THEN RETURN item.decision; END IF;
        RETURN NULL;
    END IF;
    IF promote THEN
        IF EXISTS (SELECT 1 FROM craft_queue.recoveries AS r WHERE r.finished_at IS NULL) THEN RETURN NULL; END IF;
        SELECT v.* INTO verdict FROM craft_queue.verifications AS v
        JOIN craft_queue.work AS w ON w.id = v.work_id AND w.generation = v.generation
           AND w.candidate = v.head AND w.candidate_evidence = v.digest
        WHERE w.id = item.work_id AND w.state IN ('candidate', 'released') AND v.generation = item.generation
          AND v.head = item.head AND v.digest = item.pack_digest
        ORDER BY v.sequence DESC LIMIT 1;
        IF NOT FOUND OR NOT verdict.passed OR verdict.sequence IS DISTINCT FROM expected_verdict THEN
            RETURN NULL;
        END IF;

        kind := item.proposal->>'kind';
        area := item.proposal->>'location';
        body := item.proposal->>'body';
        IF kind = 'skill' THEN
            -- Preserve the ownership and repository scope of catalog locations already in use.
            IF EXISTS (SELECT 1 FROM craft_queue.team_skill_releases AS s WHERE s.location = area
                       AND (NOT (s.repositories @> destinations AND s.repositories <@ destinations)
                            OR s.owner_name <> source_owner)) THEN
                RETURN NULL;
            END IF;
            INSERT INTO craft_queue.team_skill_releases
                (location, revision, markdown, owner_name, approval_reference, repositories)
            VALUES (area, 'sha256:' || encode(sha256(convert_to(body, 'UTF8')), 'hex'),
                body, source_owner, evidence, destinations)
            ON CONFLICT DO NOTHING RETURNING team_skill_releases.revision INTO revision;
        ELSE
            source_ref := 'craft:work/' || item.work_id || '/verification/' || verdict.sequence;
            revision := craft_queue.project_context_revision(kind, source_ref, item.head, evidence, body);
            BEGIN
                INSERT INTO craft_queue.project_context_releases (
                    repository, location, revision, excerpt, kind, source_reference, source_revision, approval_reference,
                    registered_by, registered_at
                )
                SELECT repo, area, revision, body, kind, source_ref, item.head, evidence, actor, clock_timestamp()
                FROM unnest(destinations) AS target(repo);
            EXCEPTION WHEN unique_violation THEN
                RETURN NULL;
            END;
        END IF;
        IF revision IS NULL THEN RETURN NULL; END IF;
    ELSIF expected_verdict IS NOT NULL THEN
        RETURN NULL;
    END IF;

    result := result || jsonb_build_object('decided_at', clock_timestamp(), 'release_revision', revision);
    UPDATE craft_queue.knowledge_proposals AS p SET decision = result WHERE p.proposal_id = id;
    RETURN result;
END;
$$;

CREATE OR REPLACE FUNCTION craft_queue.review_knowledge(
    actor text, id text, expected_digest text, promote boolean, source_owner text, evidence text, expected_verdict bigint
) RETURNS jsonb
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT craft_queue.review_knowledge(actor, id, expected_digest, promote, source_owner, evidence,
        expected_verdict, NULL::text[]);
$$;

REVOKE ALL ON FUNCTION craft_queue.review_knowledge(text,text,text,boolean,text,text,bigint,text[]) FROM PUBLIC;

COMMIT;
