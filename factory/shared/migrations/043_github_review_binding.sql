BEGIN;

ALTER TABLE craft_queue.draft_policies
    ADD COLUMN forge text NOT NULL DEFAULT 'bitbucket' CHECK (forge IN ('bitbucket', 'github'));

ALTER FUNCTION craft_queue.reserve_draft(text, text, bigint, text, bigint)
    RENAME TO reserve_bitbucket_draft;
ALTER FUNCTION craft_queue.reserve_draft_update(text, text, bigint, text, bigint)
    RENAME TO reserve_bitbucket_draft_update;

CREATE FUNCTION craft_queue.reserve_draft(
    actor text, order_id text, current_generation bigint, candidate_head text, review_sequence bigint
) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    IF EXISTS (SELECT 1 FROM craft_queue.work AS w
        JOIN craft_queue.draft_policies AS p ON p.repository = w.repository
        WHERE w.id = order_id AND p.forge = 'github')
       OR EXISTS (SELECT 1 FROM craft_queue.publications AS p WHERE p.work_id = order_id
          AND p.result_url LIKE 'https://github.com/%') THEN RETURN NULL; END IF;
    RETURN craft_queue.reserve_bitbucket_draft(actor, order_id, current_generation, candidate_head, review_sequence);
END;
$$;

CREATE FUNCTION craft_queue.reserve_draft_update(
    actor text, order_id text, current_generation bigint, candidate_head text, review_sequence bigint
) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    IF EXISTS (SELECT 1 FROM craft_queue.work AS w
        JOIN craft_queue.draft_policies AS p ON p.repository = w.repository
        WHERE w.id = order_id AND p.forge = 'github')
       OR EXISTS (SELECT 1 FROM craft_queue.publications AS p WHERE p.work_id = order_id
          AND p.result_url LIKE 'https://github.com/%') THEN RETURN NULL; END IF;
    RETURN craft_queue.reserve_bitbucket_draft_update(actor, order_id, current_generation, candidate_head, review_sequence);
END;
$$;

CREATE OR REPLACE FUNCTION craft_queue.prepare_draft(actor text, key text, update_draft boolean)
RETURNS jsonb
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT craft_queue.prepare_draft_receipt(actor, key)
    FROM craft_queue.publications AS p
    JOIN craft_queue.draft_policies AS policy ON policy.repository = p.repository AND policy.forge = 'bitbucket'
    WHERE p.action_key = key AND (p.prior_action_key IS NOT NULL) = update_draft;
$$;

CREATE OR REPLACE FUNCTION craft_queue.begin_draft(actor text, key text, update_draft boolean)
RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    RETURN (SELECT craft_queue.begin_draft_receipt(actor, key)
        FROM craft_queue.publications AS p
        JOIN craft_queue.draft_policies AS policy ON policy.repository = p.repository AND policy.forge = 'bitbucket'
        WHERE p.action_key = key AND (p.prior_action_key IS NOT NULL) = update_draft);
END;
$$;

CREATE FUNCTION craft_queue.bind_github_review(
    actor_id text, order_id text, candidate_generation bigint, candidate_head text,
    review_sequence bigint, observation jsonb
) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    settings craft_queue.control%ROWTYPE;
    target record;
    prior craft_queue.publications%ROWTYPE;
    existing craft_queue.publications%ROWTYPE;
    key text;
    url text;
    prefix text;
    source_ref text;
    evidence_ref text;
    active_policy boolean;
BEGIN
    SELECT * INTO STRICT settings FROM craft_queue.control FOR UPDATE;
    SELECT w.repository, w.payload->>'base' AS base, w.repair_from_generation,
        policy.base_branch, policy.publisher_id, policy.enabled, v.digest
    INTO target
    FROM craft_queue.work AS w
    JOIN craft_queue.draft_policies AS policy ON policy.repository = w.repository AND policy.forge = 'github'
    JOIN LATERAL (
        SELECT * FROM craft_queue.verifications AS verdict WHERE verdict.work_id = w.id
          AND verdict.generation = w.generation ORDER BY verdict.sequence DESC LIMIT 1
    ) AS v ON v.sequence = review_sequence AND v.passed AND v.head = w.candidate
        AND v.digest = w.candidate_evidence AND v.verifier <> actor_id
    WHERE w.id = order_id AND w.state = 'candidate' AND w.generation = candidate_generation
        AND w.candidate = candidate_head AND craft_queue.series_blocker(w.id) IS NULL;
    IF NOT FOUND THEN RETURN NULL; END IF;

    prefix := 'https://github.com/' || target.repository || '/pull/';
    source_ref := 'craft-factory/' || order_id;
    IF target.repair_from_generation IS NOT NULL THEN
        SELECT p.* INTO prior FROM craft_queue.repair_requests AS r
        JOIN craft_queue.publications AS p ON p.work_id = r.work_id AND p.generation = r.from_generation
            AND p.state = 'confirmed' AND p.result_url = r.native_url AND p.head = r.head
        WHERE r.work_id = order_id AND r.from_generation = target.repair_from_generation
            AND p.base = target.base AND p.base_branch = target.base_branch AND p.head <> candidate_head
            AND left(p.result_url, length(prefix)) = prefix;
        IF NOT FOUND THEN RETURN NULL; END IF;
        source_ref := prior.source_branch;
    END IF;

    active_policy := target.enabled AND NOT settings.paused AND NOT EXISTS (
        SELECT 1 FROM craft_queue.recoveries AS r WHERE r.finished_at IS NULL
    );
    IF observation IS NULL THEN
        IF NOT active_policy THEN RETURN NULL; END IF;
        RETURN jsonb_build_object('repository', target.repository, 'head', candidate_head,
            'base', target.base, 'source_branch', source_ref, 'base_branch', target.base_branch,
            'pack_digest', target.digest,
            'expected_draft_url', prior.result_url);
    END IF;

    url := observation->>'native_url';
    IF url IS NULL OR left(url, length(prefix)) <> prefix
        OR substring(url FROM length(prefix) + 1) !~ '^[1-9][0-9]{0,17}$'
        OR observation->>'head' IS DISTINCT FROM candidate_head
        OR observation->>'base' IS DISTINCT FROM target.base
        OR observation->>'source_branch' IS DISTINCT FROM source_ref
        OR observation->>'base_branch' IS DISTINCT FROM target.base_branch
        OR (prior.action_key IS NOT NULL AND url <> prior.result_url) THEN RETURN NULL; END IF;
    evidence_ref := 'GitHub observed open draft ' || url || ' on the pinned source and base';

    SELECT * INTO existing FROM craft_queue.publications AS p
    WHERE p.work_id = order_id AND p.generation = candidate_generation;
    IF FOUND THEN
        IF existing.state = 'confirmed' AND existing.head = candidate_head
            AND existing.verification_sequence = review_sequence AND existing.pack_digest = target.digest
            AND existing.result_url = url AND EXISTS (
                SELECT 1 FROM craft_queue.publication_events AS e WHERE e.action_key = existing.action_key
                    AND e.state = 'confirmed' AND e.actor = actor_id AND e.evidence = evidence_ref
            ) THEN RETURN craft_queue.publication_status(existing.action_key); END IF;
        RETURN NULL;
    END IF;
    IF NOT active_policy THEN RETURN NULL; END IF;

    key := 'github-review:' || encode(sha256(convert_to(jsonb_build_array(
        target.repository, order_id, candidate_generation, candidate_head
    )::text, 'UTF8')), 'hex');
    INSERT INTO craft_queue.publications (action_key, work_id, generation, verification_sequence,
        head, base, pack_digest, repository, base_branch, source_branch, publisher_id, state,
        result_url, prior_action_key, repair_head, expected_draft_url)
    VALUES (key, order_id, candidate_generation, review_sequence, candidate_head, target.base,
        target.digest, target.repository, target.base_branch, source_ref, target.publisher_id, 'confirmed',
        url, prior.action_key, prior.head, prior.result_url)
    ON CONFLICT DO NOTHING RETURNING * INTO existing;
    IF NOT FOUND THEN RETURN NULL; END IF;
    INSERT INTO craft_queue.publication_events (action_key, state, actor, evidence)
    VALUES (key, 'confirmed', actor_id, evidence_ref);
    RETURN craft_queue.publication_status(key);
END;
$$;

REVOKE ALL ON FUNCTION craft_queue.reserve_draft(text, text, bigint, text, bigint) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.reserve_draft_update(text, text, bigint, text, bigint) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.bind_github_review(text, text, bigint, text, bigint, jsonb) FROM PUBLIC;
COMMIT;
