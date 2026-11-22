BEGIN;

CREATE TABLE craft_queue.knowledge_proposals (
    proposal_id text PRIMARY KEY,
    work_id text NOT NULL,
    generation bigint NOT NULL,
    repository text NOT NULL,
    head text NOT NULL,
    pack_digest text NOT NULL,
    proposed_by text NOT NULL,
    proposed_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    proposal jsonb NOT NULL,
    proposal_digest text NOT NULL,
    decision jsonb,
    FOREIGN KEY (work_id, generation) REFERENCES craft_queue.attempts (work_id, generation)
);
CREATE UNIQUE INDEX knowledge_proposals_per_location
    ON craft_queue.knowledge_proposals (work_id, generation, (proposal->>'location'));
CREATE INDEX knowledge_proposals_pending
    ON craft_queue.knowledge_proposals (repository, proposed_at, proposal_id) WHERE decision IS NULL;

CREATE FUNCTION craft_queue.propose_knowledge(
    actor text, id text, order_id text, candidate_generation bigint, candidate_head text, content jsonb
) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    item craft_queue.knowledge_proposals%ROWTYPE;
    source craft_queue.work%ROWTYPE;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    IF COALESCE(id, '') !~ '^[A-Za-z0-9_-]{1,100}$'
        OR COALESCE(content->>'kind', '') NOT IN ('skill', 'decision', 'runbook')
        OR COALESCE(content->>'location', '') !~ (
            CASE WHEN content->>'kind' = 'skill' THEN '^team:' ELSE '^project:' END
            || '[a-z0-9][a-z0-9_-]{0,79}$')
        OR length(btrim(COALESCE(content->>'body', ''))) = 0
        OR octet_length(convert_to(content->>'body', 'UTF8')) >
            (CASE WHEN content->>'kind' = 'skill' THEN 32768 ELSE 4096 END)
        OR length(btrim(COALESCE(content->>'evidence', ''))) NOT BETWEEN 1 AND 500
        OR content->>'evidence' ~ '[[:cntrl:]]' THEN RETURN NULL; END IF;

    SELECT w.* INTO source FROM craft_queue.work AS w
    JOIN craft_queue.attempts AS a ON a.work_id = w.id AND a.generation = w.generation
    JOIN craft_queue.runner_repositories AS g ON g.repository = w.repository AND g.runner_id = a.runner_id
    WHERE w.id = order_id AND w.generation = candidate_generation AND w.candidate = candidate_head
      AND w.state IN ('candidate', 'released') AND a.runner_id = actor AND a.outcome = 'candidate'
      AND EXISTS (SELECT 1 FROM craft_queue.candidate_packs AS p
                  WHERE p.work_id = w.id AND p.generation = w.generation AND p.digest = w.candidate_evidence);
    IF NOT FOUND THEN RETURN NULL; END IF;

    SELECT * INTO item FROM craft_queue.knowledge_proposals AS p WHERE p.proposal_id = id;
    IF FOUND THEN
        IF item.proposed_by = actor AND item.work_id = order_id AND item.generation = candidate_generation
            AND item.head = candidate_head AND item.proposal = content THEN
            RETURN to_jsonb(item) - 'proposal' - 'decision';
        END IF;
        RETURN NULL;
    END IF;
    IF (SELECT count(*) FROM craft_queue.knowledge_proposals AS p
        WHERE p.work_id = order_id AND p.generation = candidate_generation) >= 4
        OR (SELECT count(*) FROM craft_queue.knowledge_proposals AS p
            WHERE p.repository = source.repository AND p.decision IS NULL) >= 20 THEN RETURN NULL; END IF;

    INSERT INTO craft_queue.knowledge_proposals
        (proposal_id, work_id, generation, repository, head, pack_digest, proposed_by, proposal, proposal_digest)
    VALUES (id, order_id, candidate_generation, source.repository, candidate_head, source.candidate_evidence, actor,
        content, 'sha256:' || encode(sha256(convert_to(content::text, 'UTF8')), 'hex'))
    ON CONFLICT DO NOTHING RETURNING * INTO item;
    IF NOT FOUND THEN RETURN NULL; END IF;
    RETURN to_jsonb(item) - 'proposal' - 'decision';
END;
$$;

CREATE FUNCTION craft_queue.knowledge_proposal(id text) RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT to_jsonb(p) FROM craft_queue.knowledge_proposals AS p WHERE p.proposal_id = id;
$$;

CREATE FUNCTION craft_queue.pending_knowledge(repo text) RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT COALESCE(jsonb_agg(item ORDER BY proposed_at, proposal_id), '[]'::jsonb)
    FROM (
        SELECT p.proposed_at, p.proposal_id, jsonb_build_object(
            'proposal_id', p.proposal_id, 'work_id', p.work_id, 'generation', p.generation,
            'head', p.head, 'kind', p.proposal->>'kind', 'location', p.proposal->>'location',
            'proposal_digest', p.proposal_digest, 'proposed_at', p.proposed_at) AS item
        FROM craft_queue.knowledge_proposals AS p
        WHERE p.repository = repo AND p.decision IS NULL
        ORDER BY p.proposed_at, p.proposal_id LIMIT 20
    ) AS pending;
$$;

CREATE FUNCTION craft_queue.review_knowledge(
    actor text, id text, expected_digest text, promote boolean, source_owner text, evidence text, expected_verdict bigint
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
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    IF promote IS NULL OR length(btrim(COALESCE(source_owner, ''))) NOT BETWEEN 1 AND 200
        OR length(btrim(COALESCE(evidence, ''))) NOT BETWEEN 1 AND 500
        OR source_owner ~ '[[:cntrl:]]' OR evidence ~ '[[:cntrl:]]' THEN RETURN NULL; END IF;
    SELECT * INTO item FROM craft_queue.knowledge_proposals AS p WHERE p.proposal_id = id;
    IF NOT FOUND OR item.proposal_digest IS DISTINCT FROM expected_digest OR item.proposed_by = actor THEN
        RETURN NULL;
    END IF;
    result := jsonb_build_object('promoted', promote, 'recorded_by', actor, 'owner', source_owner,
        'evidence', evidence, 'verification_sequence', expected_verdict);
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
                       AND (s.repositories <> ARRAY[item.repository] OR s.owner_name <> source_owner)) THEN
                RETURN NULL;
            END IF;
            INSERT INTO craft_queue.team_skill_releases
                (location, revision, markdown, owner_name, approval_reference, repositories)
            VALUES (area, 'sha256:' || encode(sha256(convert_to(body, 'UTF8')), 'hex'),
                body, source_owner, evidence, ARRAY[item.repository])
            ON CONFLICT DO NOTHING RETURNING team_skill_releases.revision INTO revision;
        ELSE
            source_ref := 'craft:work/' || item.work_id || '/verification/' || verdict.sequence;
            INSERT INTO craft_queue.project_context_releases
                (repository, location, revision, excerpt, kind, source_reference, source_revision, approval_reference)
            VALUES (item.repository, area, craft_queue.project_context_revision(kind, source_ref, item.head, evidence, body),
                body, kind, source_ref, item.head, evidence)
            ON CONFLICT DO NOTHING RETURNING project_context_releases.revision INTO revision;
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

REVOKE ALL ON FUNCTION craft_queue.propose_knowledge(text, text, text, bigint, text, jsonb) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.knowledge_proposal(text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.pending_knowledge(text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.review_knowledge(text, text, text, boolean, text, text, bigint) FROM PUBLIC;

COMMIT;
