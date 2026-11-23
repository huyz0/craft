BEGIN;

ALTER TABLE craft_queue.publications
    ADD COLUMN prior_action_key text REFERENCES craft_queue.publications(action_key),
    ADD COLUMN repair_head text,
    ADD COLUMN expected_draft_url text,
    ADD CONSTRAINT publication_update_target CHECK (
        (prior_action_key IS NULL AND repair_head IS NULL AND expected_draft_url IS NULL)
        OR (prior_action_key IS NOT NULL AND repair_head IS NOT NULL AND expected_draft_url IS NOT NULL
            AND repair_head <> head AND (result_url IS NULL OR result_url = expected_draft_url))
    ),
    DROP CONSTRAINT publications_repository_source_branch_key,
    DROP CONSTRAINT publications_result_url_key;

CREATE UNIQUE INDEX publication_draft_creation_branch
    ON craft_queue.publications(repository, source_branch) WHERE prior_action_key IS NULL;
CREATE UNIQUE INDEX publication_draft_creation_url
    ON craft_queue.publications(result_url) WHERE prior_action_key IS NULL;
CREATE UNIQUE INDEX publication_draft_update_parent
    ON craft_queue.publications(prior_action_key) WHERE prior_action_key IS NOT NULL;

CREATE OR REPLACE FUNCTION craft_queue.reserve_draft(
    actor text, order_id text, current_generation bigint, candidate_head text, review_sequence bigint
) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    IF EXISTS (
        SELECT 1 FROM craft_queue.work AS w
        WHERE w.id = order_id AND w.repair_from_generation IS NOT NULL
          AND NOT EXISTS (
              SELECT 1 FROM craft_queue.publications AS p
              WHERE p.work_id = w.id AND p.generation = current_generation AND p.prior_action_key IS NULL
          )
    ) THEN RETURN NULL; END IF;
    RETURN craft_queue.reserve_new_draft(actor, order_id, current_generation, candidate_head, review_sequence);
END;
$$;

CREATE FUNCTION craft_queue.reserve_draft_update(
    operator_id text, order_id text, current_generation bigint, candidate_head text, review_sequence bigint
) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    settings craft_queue.control%ROWTYPE;
    target record;
    existing craft_queue.publications%ROWTYPE;
    key text;
BEGIN
    SELECT * INTO STRICT settings FROM craft_queue.control FOR UPDATE;
    SELECT * INTO existing FROM craft_queue.publications AS p
    WHERE p.work_id = order_id AND p.generation = current_generation;
    IF FOUND THEN
        IF existing.prior_action_key IS NULL OR existing.head IS DISTINCT FROM candidate_head
           OR existing.verification_sequence IS DISTINCT FROM review_sequence THEN RETURN NULL; END IF;
        RETURN craft_queue.publication_status(existing.action_key);
    END IF;
    IF settings.paused THEN RETURN NULL; END IF;

    SELECT w.repository, p.base, p.base_branch, p.source_branch, policy.publisher_id,
           p.action_key AS prior_action_key, p.head AS repair_head, p.result_url AS draft_url, v.digest
    INTO target
    FROM craft_queue.work AS w
    JOIN craft_queue.repair_requests AS r
      ON r.work_id = w.id AND r.from_generation = w.repair_from_generation
    JOIN craft_queue.publications AS p
      ON p.work_id = r.work_id AND p.generation = r.from_generation AND p.state = 'confirmed'
     AND p.head = r.head AND p.result_url = r.native_url AND p.base = w.payload->>'base'
    JOIN craft_queue.draft_policies AS policy
      ON policy.repository = w.repository AND policy.enabled AND policy.base_branch = p.base_branch
    JOIN LATERAL (
        SELECT * FROM craft_queue.verifications AS verdict
        WHERE verdict.work_id = w.id AND verdict.generation = w.generation
        ORDER BY verdict.sequence DESC LIMIT 1
    ) AS v ON true
    WHERE w.id = order_id AND w.generation = current_generation AND w.state = 'candidate'
      AND w.candidate = candidate_head AND w.candidate <> r.head AND v.passed
      AND v.sequence = review_sequence AND v.head = w.candidate AND v.digest = w.candidate_evidence
      AND policy.publisher_id <> operator_id AND policy.publisher_id <> v.verifier;
    IF NOT FOUND THEN RETURN NULL; END IF;

    key := 'draft-update:' || encode(sha256(convert_to(jsonb_build_array(
        target.prior_action_key, current_generation, candidate_head
    )::text, 'UTF8')), 'hex');
    INSERT INTO craft_queue.publications (
        action_key, work_id, generation, verification_sequence, head, base, pack_digest,
        repository, base_branch, source_branch, publisher_id, state, prior_action_key, repair_head, expected_draft_url
    ) VALUES (
        key, order_id, current_generation, review_sequence, candidate_head, target.base, target.digest,
        target.repository, target.base_branch, target.source_branch, target.publisher_id, 'reserved',
        target.prior_action_key, target.repair_head, target.draft_url
    );
    INSERT INTO craft_queue.publication_events (action_key, state, actor, evidence)
    VALUES (key, 'reserved', operator_id, 'Independent repair verification sequence ' || review_sequence::text);
    RETURN craft_queue.publication_status(key);
END;
$$;

ALTER FUNCTION craft_queue.prepare_draft(text, text) RENAME TO prepare_draft_receipt;
ALTER FUNCTION craft_queue.begin_draft(text, text) RENAME TO begin_draft_receipt;

CREATE FUNCTION craft_queue.prepare_draft(actor text, key text, update_draft boolean)
RETURNS jsonb
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT craft_queue.prepare_draft_receipt(actor, key)
    FROM craft_queue.publications AS p
    WHERE p.action_key = key AND (p.prior_action_key IS NOT NULL) = update_draft;
$$;

CREATE FUNCTION craft_queue.prepare_draft(actor text, key text) RETURNS jsonb
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT craft_queue.prepare_draft(actor, key, false);
$$;

CREATE FUNCTION craft_queue.begin_draft(actor text, key text, update_draft boolean)
RETURNS jsonb
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT craft_queue.begin_draft_receipt(actor, key)
    FROM craft_queue.publications AS p
    WHERE p.action_key = key AND (p.prior_action_key IS NOT NULL) = update_draft;
$$;

CREATE FUNCTION craft_queue.begin_draft(actor text, key text) RETURNS jsonb
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT craft_queue.begin_draft(actor, key, false);
$$;

ALTER FUNCTION craft_queue.record_draft_outcome(text, text, boolean, text, text, text, text, text, text)
    RENAME TO record_draft_receipt;
CREATE FUNCTION craft_queue.record_draft_outcome(
    actor text, key text, is_operator boolean, outcome text, observed_head text,
    native_url text, observed_source text, observed_base text, evidence_ref text
) RETURNS jsonb
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT craft_queue.record_draft_receipt(
        actor, key, is_operator, outcome, observed_head, native_url, observed_source, observed_base, evidence_ref
    )
    FROM craft_queue.publications AS p
    WHERE p.action_key = key
      AND (p.prior_action_key IS NULL OR outcome <> 'confirmed' OR p.expected_draft_url = native_url);
$$;

REVOKE ALL ON FUNCTION craft_queue.reserve_draft_update(text, text, bigint, text, bigint) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.prepare_draft(text, text, boolean) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.prepare_draft(text, text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.begin_draft(text, text, boolean) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.begin_draft(text, text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.record_draft_outcome(text, text, boolean, text, text, text, text, text, text)
    FROM PUBLIC;
COMMIT;
