BEGIN;

CREATE TABLE craft_queue.draft_policies (
    repository text PRIMARY KEY CHECK (repository ~ '^[a-z0-9_-]+/[a-z0-9_-]+$'),
    base_branch text NOT NULL CHECK (base_branch ~ '^[A-Za-z0-9][A-Za-z0-9_./-]{0,149}$'),
    publisher_id text NOT NULL CHECK (publisher_id ~ '^[A-Za-z0-9_-]{1,100}$'),
    enabled boolean NOT NULL DEFAULT false
);

CREATE TABLE craft_queue.publications (
    action_key text PRIMARY KEY,
    work_id text NOT NULL,
    generation bigint NOT NULL,
    verification_sequence bigint NOT NULL REFERENCES craft_queue.verifications(sequence),
    head text NOT NULL,
    base text NOT NULL,
    pack_digest text NOT NULL,
    repository text NOT NULL,
    base_branch text NOT NULL,
    source_branch text NOT NULL,
    publisher_id text NOT NULL,
    state text NOT NULL CHECK (state IN ('reserved','sending','uncertain','confirmed','absent')),
    result_url text UNIQUE,
    UNIQUE (work_id, generation),
    UNIQUE (repository, source_branch),
    FOREIGN KEY (work_id, generation) REFERENCES craft_queue.candidate_packs(work_id, generation)
);

CREATE TABLE craft_queue.publication_events (
    sequence bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    action_key text NOT NULL REFERENCES craft_queue.publications(action_key),
    state text NOT NULL,
    actor text NOT NULL,
    evidence text NOT NULL,
    observed_at timestamptz NOT NULL DEFAULT clock_timestamp()
);

CREATE FUNCTION craft_queue.publication_status(key text) RETURNS jsonb
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT to_jsonb(p) || jsonb_build_object('events', COALESCE((
        SELECT jsonb_agg(to_jsonb(e) ORDER BY e.sequence)
        FROM craft_queue.publication_events AS e WHERE e.action_key = p.action_key
    ), '[]'::jsonb))
    FROM craft_queue.publications AS p WHERE p.action_key = key;
$$;

CREATE FUNCTION craft_queue.reserve_draft(
    actor text, order_id text, current_generation bigint, candidate_head text, review_sequence bigint
) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    settings craft_queue.control%ROWTYPE;
    candidate craft_queue.work%ROWTYPE;
    policy craft_queue.draft_policies%ROWTYPE;
    verdict craft_queue.verifications%ROWTYPE;
    previous craft_queue.publications%ROWTYPE;
    key text;
BEGIN
    SELECT * INTO STRICT settings FROM craft_queue.control FOR UPDATE;
    SELECT * INTO previous FROM craft_queue.publications AS p
    WHERE p.work_id = order_id AND p.generation = current_generation;
    IF FOUND THEN
        IF previous.head IS DISTINCT FROM candidate_head
           OR previous.verification_sequence IS DISTINCT FROM review_sequence THEN
            RETURN NULL;
        END IF;
        RETURN craft_queue.publication_status(previous.action_key);
    END IF;
    IF settings.paused THEN RETURN NULL; END IF;
    SELECT * INTO candidate FROM craft_queue.work AS w
    WHERE w.id = order_id AND w.state = 'candidate'
      AND w.generation = current_generation AND w.candidate = candidate_head;
    IF NOT FOUND THEN RETURN NULL; END IF;
    SELECT * INTO policy FROM craft_queue.draft_policies AS p
    WHERE p.repository = candidate.repository AND p.enabled;
    IF NOT FOUND OR policy.publisher_id = actor THEN RETURN NULL; END IF;
    SELECT * INTO verdict FROM craft_queue.verifications AS v
    WHERE v.work_id = order_id AND v.generation = current_generation
    ORDER BY v.sequence DESC LIMIT 1;
    IF NOT FOUND OR NOT verdict.passed OR verdict.sequence <> review_sequence
       OR verdict.head <> candidate.candidate OR verdict.digest <> candidate.candidate_evidence
       OR verdict.verifier = policy.publisher_id THEN
        RETURN NULL;
    END IF;
    key := 'draft:' || encode(sha256(convert_to(jsonb_build_array(
        candidate.repository, order_id, current_generation, candidate_head
    )::text, 'UTF8')), 'hex');
    INSERT INTO craft_queue.publications (
        action_key, work_id, generation, verification_sequence, head, base, pack_digest,
        repository, base_branch, source_branch, publisher_id, state
    ) VALUES (
        key, order_id, current_generation, review_sequence, candidate_head, candidate.payload->>'base',
        verdict.digest, candidate.repository, policy.base_branch, 'craft-factory/' || order_id,
        policy.publisher_id, 'reserved'
    );
    INSERT INTO craft_queue.publication_events (action_key, state, actor, evidence)
    VALUES (key, 'reserved', actor, 'Independent verification sequence ' || review_sequence::text);
    RETURN craft_queue.publication_status(key);
END;
$$;

CREATE FUNCTION craft_queue.begin_draft(actor text, key text) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    settings craft_queue.control%ROWTYPE;
    publication craft_queue.publications%ROWTYPE;
BEGIN
    SELECT * INTO STRICT settings FROM craft_queue.control FOR UPDATE;
    IF settings.paused THEN RETURN NULL; END IF;
    SELECT p.* INTO publication FROM craft_queue.publications AS p
    JOIN craft_queue.draft_policies AS policy ON policy.repository = p.repository
    JOIN craft_queue.work AS w ON w.id = p.work_id
    JOIN LATERAL (
        SELECT v.sequence, v.passed FROM craft_queue.verifications AS v
        WHERE v.work_id = p.work_id AND v.generation = p.generation
        ORDER BY v.sequence DESC LIMIT 1
    ) AS latest ON true
    WHERE p.action_key = key AND p.state = 'reserved'
      AND p.publisher_id = actor AND policy.publisher_id = actor AND policy.enabled
      AND policy.base_branch = p.base_branch
      AND latest.passed AND latest.sequence = p.verification_sequence
      AND w.state = 'candidate' AND w.generation = p.generation AND w.candidate = p.head
      AND w.candidate_evidence = p.pack_digest;
    IF NOT FOUND THEN RETURN NULL; END IF;
    UPDATE craft_queue.publications AS p SET state = 'sending' WHERE p.action_key = key;
    INSERT INTO craft_queue.publication_events (action_key, state, actor, evidence)
    VALUES (key, 'sending', actor, 'External send reserved');
    RETURN to_jsonb(publication) || jsonb_build_object('state', 'sending');
END;
$$;

CREATE FUNCTION craft_queue.record_draft_outcome(
    actor text, key text, is_operator boolean, outcome text, observed_head text,
    native_url text, observed_source text, observed_base text, evidence_ref text
) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    publication craft_queue.publications%ROWTYPE;
    previous craft_queue.publication_events%ROWTYPE;
    url_prefix text;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    SELECT * INTO publication FROM craft_queue.publications AS p WHERE p.action_key = key;
    IF NOT FOUND OR (NOT is_operator AND publication.publisher_id <> actor)
       OR COALESCE(outcome, '') NOT IN ('uncertain', 'confirmed', 'absent')
       OR length(btrim(COALESCE(evidence_ref,''))) NOT BETWEEN 1 AND 1000
       OR publication.head IS DISTINCT FROM observed_head THEN
        RETURN NULL;
    END IF;
    IF outcome = 'confirmed' THEN
        url_prefix := 'https://bitbucket.org/' || publication.repository || '/pull-requests/';
        IF COALESCE(left(native_url, length(url_prefix)), '') <> url_prefix
            OR substring(native_url FROM length(url_prefix) + 1) !~ '^[1-9][0-9]{0,17}$'
            OR observed_source IS DISTINCT FROM publication.source_branch
            OR observed_base IS DISTINCT FROM publication.base_branch THEN
            RETURN NULL;
        END IF;
    ELSIF native_url IS NOT NULL OR observed_source IS NOT NULL OR observed_base IS NOT NULL THEN
        RETURN NULL;
    END IF;
    IF publication.state = outcome THEN
        SELECT e.* INTO previous FROM craft_queue.publication_events AS e
        WHERE e.action_key = key ORDER BY e.sequence DESC LIMIT 1;
        IF previous.actor = actor AND previous.evidence = evidence_ref
           AND publication.result_url IS NOT DISTINCT FROM native_url THEN
            RETURN craft_queue.publication_status(key);
        END IF;
        RETURN NULL;
    END IF;
    IF (outcome = 'uncertain' AND publication.state <> 'sending')
        OR (outcome = 'confirmed' AND publication.state NOT IN ('sending','uncertain'))
        OR (outcome = 'absent' AND (NOT is_operator OR publication.state NOT IN ('reserved','sending','uncertain')))
        OR (NOT is_operator AND publication.state <> 'sending') THEN
        RETURN NULL;
    END IF;
    UPDATE craft_queue.publications AS p SET state = outcome, result_url = native_url WHERE p.action_key = key;
    INSERT INTO craft_queue.publication_events (action_key, state, actor, evidence)
    VALUES (key, outcome, actor, evidence_ref);
    RETURN craft_queue.publication_status(key);
END;
$$;

CREATE OR REPLACE FUNCTION craft_queue.cancel(work_id text, reason text) RETURNS boolean
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    selected craft_queue.work%ROWTYPE;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    IF length(btrim(COALESCE(reason, ''))) NOT BETWEEN 1 AND 500 THEN
        RAISE EXCEPTION 'a cancellation reason is required';
    END IF;
    SELECT * INTO selected FROM craft_queue.work AS w WHERE w.id = work_id;
    IF NOT FOUND THEN RETURN false; END IF;
    IF selected.state = 'cancelled' THEN RETURN selected.cancellation_reason = reason; END IF;
    IF EXISTS (
        SELECT 1 FROM craft_queue.publications AS p
        WHERE p.work_id = selected.id AND p.state IN ('reserved','sending','uncertain')
    ) THEN
        RAISE EXCEPTION 'reconcile the publication before cancelling the work';
    END IF;
    UPDATE craft_queue.attempts AS a SET outcome = 'cancelled'
    WHERE a.work_id = selected.id AND a.generation = selected.generation AND a.outcome = 'leased';
    UPDATE craft_queue.work AS w SET state = 'cancelled', cancellation_reason = reason WHERE w.id = selected.id;
    RETURN true;
END;
$$;

REVOKE ALL ON FUNCTION craft_queue.publication_status(text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.reserve_draft(text, text, bigint, text, bigint) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.begin_draft(text, text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.record_draft_outcome(text, text, boolean, text, text, text, text, text, text) FROM PUBLIC;

COMMIT;
