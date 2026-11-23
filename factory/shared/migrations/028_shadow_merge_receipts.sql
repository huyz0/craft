BEGIN;

CREATE TABLE craft_queue.shadow_policies (
    repository text NOT NULL,
    digest text NOT NULL CHECK (digest ~ '^sha256:[a-f0-9]{64}$'),
    require_native boolean NOT NULL,
    actor text NOT NULL,
    evidence text NOT NULL,
    approved_at timestamptz NOT NULL,
    expires_at timestamptz NOT NULL CHECK (expires_at > approved_at),
    PRIMARY KEY (repository, digest)
);

CREATE TABLE craft_queue.shadow_merge_receipts (
    id text PRIMARY KEY,
    work_id text NOT NULL,
    generation bigint NOT NULL,
    verification_sequence bigint NOT NULL REFERENCES craft_queue.verifications(sequence),
    publication_key text NOT NULL REFERENCES craft_queue.publications(action_key),
    head text NOT NULL,
    pack_digest text NOT NULL,
    repository text NOT NULL,
    policy_digest text NOT NULL,
    report_digest text NOT NULL CHECK (report_digest ~ '^sha256:[a-f0-9]{64}$'),
    assessment jsonb NOT NULL,
    actor text NOT NULL,
    recorded_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    UNIQUE (work_id, generation, policy_digest, report_digest),
    FOREIGN KEY (work_id, generation) REFERENCES craft_queue.candidate_packs(work_id, generation),
    FOREIGN KEY (repository, policy_digest) REFERENCES craft_queue.shadow_policies(repository, digest)
);
CREATE INDEX shadow_merge_receipts_by_repository ON craft_queue.shadow_merge_receipts (repository, recorded_at DESC);

CREATE FUNCTION craft_queue.approve_shadow_policy(
    actor_id text, repo text, pin text, native_required boolean, evidence_ref text, valid_seconds integer
) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    approved craft_queue.shadow_policies%ROWTYPE;
    now_at timestamptz;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    IF length(btrim(COALESCE(repo, ''))) NOT BETWEEN 1 AND 200
       OR COALESCE(pin, '') !~ '^sha256:[a-f0-9]{64}$' OR native_required IS NULL
       OR length(btrim(COALESCE(evidence_ref, ''))) NOT BETWEEN 1 AND 500
       OR COALESCE(valid_seconds, 0) NOT BETWEEN 1 AND 2592000 THEN RETURN NULL; END IF;
    SELECT * INTO approved FROM craft_queue.shadow_policies AS p WHERE p.repository = repo AND p.digest = pin;
    IF FOUND THEN
        IF approved.require_native = native_required AND approved.actor = actor_id AND approved.evidence = evidence_ref
            AND approved.expires_at - approved.approved_at = make_interval(secs => valid_seconds)
        THEN RETURN to_jsonb(approved); END IF;
        RETURN NULL;
    END IF;
    IF EXISTS (SELECT 1 FROM craft_queue.recoveries AS r WHERE r.finished_at IS NULL) THEN RETURN NULL; END IF;
    now_at := clock_timestamp();
    INSERT INTO craft_queue.shadow_policies (repository, digest, require_native, actor, evidence, approved_at, expires_at)
    VALUES (repo, pin, native_required, actor_id, evidence_ref, now_at, now_at + make_interval(secs => valid_seconds))
    RETURNING * INTO approved;
    RETURN to_jsonb(approved);
END;
$$;

CREATE FUNCTION craft_queue.record_shadow_merge(
    actor_id text, receipt_id text, order_id text, candidate_generation bigint, candidate_head text,
    candidate_digest text, review_sequence bigint, digest text, assessment_data jsonb
) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    previous craft_queue.shadow_merge_receipts%ROWTYPE;
    work craft_queue.work%ROWTYPE;
    policy craft_queue.shadow_policies%ROWTYPE;
    draft craft_queue.publications%ROWTYPE;
    now_at timestamptz;
    checked_at bigint;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    SELECT * INTO previous FROM craft_queue.shadow_merge_receipts AS r WHERE r.id = receipt_id;
    IF FOUND THEN
        IF previous.actor = actor_id AND previous.work_id = order_id AND previous.generation = candidate_generation
            AND previous.head = candidate_head AND previous.pack_digest = candidate_digest
            AND previous.verification_sequence = review_sequence AND previous.report_digest = digest
            AND previous.assessment = assessment_data THEN RETURN to_jsonb(previous); END IF;
        RETURN NULL;
    END IF;

    SELECT w.* INTO work FROM craft_queue.work AS w
    JOIN craft_queue.candidate_packs AS p ON p.work_id = w.id AND p.generation = w.generation
    WHERE w.id = order_id AND w.state = 'candidate' AND w.generation = candidate_generation
        AND w.candidate = candidate_head AND w.candidate_evidence = candidate_digest AND p.digest = candidate_digest;
    IF NOT FOUND OR work.repository IS DISTINCT FROM assessment_data->>'repository'
        OR work.payload->>'base' IS DISTINCT FROM assessment_data->>'base'
        OR review_sequence IS DISTINCT FROM (SELECT max(v.sequence) FROM craft_queue.verifications AS v
            WHERE v.work_id = order_id AND v.generation = candidate_generation AND v.head = candidate_head
                AND v.digest = candidate_digest) THEN RETURN NULL; END IF;
    SELECT * INTO draft FROM craft_queue.publications AS p
    WHERE p.work_id = order_id AND p.generation = candidate_generation AND p.head = candidate_head
        AND p.pack_digest = candidate_digest AND p.state = 'confirmed' AND p.result_url IS NOT NULL;
    IF NOT FOUND THEN RETURN NULL; END IF;

    now_at := clock_timestamp();
    IF assessment_data->>'as_of' !~ '^[0-9]{1,12}$' THEN RETURN NULL; END IF;
    checked_at := (assessment_data->>'as_of')::bigint;
    SELECT * INTO policy FROM craft_queue.shadow_policies AS p
    WHERE p.repository = work.repository AND p.digest = assessment_data->>'policy_digest'
        AND p.expires_at > now_at AND floor(extract(epoch FROM p.approved_at)) <= checked_at;
    IF NOT FOUND OR checked_at IS NULL OR checked_at NOT BETWEEN
        floor(extract(epoch FROM now_at)) - 600 AND floor(extract(epoch FROM now_at))
        OR EXISTS (SELECT 1 FROM craft_queue.recoveries AS r WHERE r.finished_at IS NULL)
        OR (assessment_data->'eligible' = 'true'::jsonb AND (
            (policy.require_native AND assessment_data->'native_verified' IS DISTINCT FROM 'true'::jsonb)
            OR assessment_data->>'pull_request' IS DISTINCT FROM draft.result_url
            OR craft_queue.current_review_draft(order_id, candidate_generation, candidate_head, review_sequence) IS NULL
        )) THEN RETURN NULL; END IF;

    INSERT INTO craft_queue.shadow_merge_receipts (id, work_id, generation, verification_sequence, publication_key,
        head, pack_digest, repository, policy_digest, report_digest, assessment, actor)
    VALUES (receipt_id, order_id, candidate_generation, review_sequence, draft.action_key, candidate_head,
        candidate_digest, work.repository, policy.digest, digest, assessment_data, actor_id)
    ON CONFLICT DO NOTHING RETURNING * INTO previous;
    IF NOT FOUND THEN RETURN NULL; END IF;
    RETURN to_jsonb(previous);
END;
$$;

CREATE FUNCTION craft_queue.shadow_merge_summary(repo text, since_epoch bigint) RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    WITH sample AS MATERIALIZED (
        SELECT r.* FROM craft_queue.shadow_merge_receipts AS r
        WHERE r.repository = repo AND r.recorded_at >= to_timestamp(since_epoch::double precision)
    )
    SELECT jsonb_build_object(
        'repository', repo, 'since', since_epoch, 'total', (SELECT count(*) FROM sample),
        'eligible', (SELECT count(*) FROM sample WHERE assessment->'eligible' = 'true'::jsonb),
        'native_verified', (SELECT count(*) FROM sample WHERE assessment->'native_verified' = 'true'::jsonb),
        'blockers', COALESCE((SELECT jsonb_object_agg(reason, total) FROM (
            SELECT b.reason, count(*) AS total FROM sample
            CROSS JOIN LATERAL jsonb_array_elements_text(assessment->'blockers') AS b(reason)
            GROUP BY b.reason
        ) AS counts), '{}'::jsonb),
        'recent', COALESCE((SELECT jsonb_agg(to_jsonb(recent) ORDER BY recorded_at DESC, id) FROM (
            SELECT * FROM sample ORDER BY recorded_at DESC, id LIMIT 20
        ) AS recent), '[]'::jsonb)
    );
$$;

CREATE FUNCTION craft_queue.shadow_merge_receipts_for(order_id text) RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT COALESCE(jsonb_agg(to_jsonb(recent) ORDER BY recorded_at DESC, id), '[]'::jsonb)
    FROM (SELECT r.* FROM craft_queue.shadow_merge_receipts AS r WHERE r.work_id = order_id
        ORDER BY r.recorded_at DESC, r.id LIMIT 20) AS recent;
$$;

REVOKE ALL ON FUNCTION craft_queue.approve_shadow_policy(text, text, text, boolean, text, integer) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.record_shadow_merge(text, text, text, bigint, text, text, bigint, text, jsonb) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.shadow_merge_summary(text, bigint) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.shadow_merge_receipts_for(text) FROM PUBLIC;
COMMIT;
