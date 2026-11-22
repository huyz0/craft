BEGIN;

ALTER TABLE craft_queue.work ADD COLUMN repair_from_generation bigint;

CREATE TABLE craft_queue.repair_requests (
    work_id text NOT NULL,
    from_generation bigint NOT NULL,
    head text NOT NULL,
    native_url text NOT NULL,
    actor text NOT NULL,
    evidence text NOT NULL,
    note text NOT NULL,
    recorded_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (work_id, from_generation),
    FOREIGN KEY (work_id, from_generation) REFERENCES craft_queue.candidate_packs(work_id, generation)
);
ALTER TABLE craft_queue.work ADD FOREIGN KEY (id, repair_from_generation)
    REFERENCES craft_queue.repair_requests(work_id, from_generation);

CREATE FUNCTION craft_queue.request_repair(
    actor_id text, order_id text, source_generation bigint, reviewed_head text,
    draft_url text, evidence_ref text, feedback text
) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    current_work craft_queue.work%ROWTYPE;
    previous craft_queue.repair_requests%ROWTYPE;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    IF length(btrim(COALESCE(evidence_ref, ''))) NOT BETWEEN 1 AND 500
       OR length(btrim(COALESCE(feedback, ''))) NOT BETWEEN 1 AND 4000 THEN
        RAISE EXCEPTION 'repair requires bounded feedback and an evidence reference';
    END IF;
    SELECT * INTO previous FROM craft_queue.repair_requests AS r
    WHERE r.work_id = order_id AND r.from_generation = source_generation;
    IF FOUND THEN
        IF previous.head = reviewed_head AND previous.native_url = draft_url AND previous.actor = actor_id
           AND previous.evidence = evidence_ref AND previous.note = feedback THEN
            RETURN to_jsonb(previous);
        END IF;
        RETURN NULL;
    END IF;
    SELECT * INTO current_work FROM craft_queue.work AS w
    WHERE w.id = order_id AND w.state = 'candidate' AND w.generation = source_generation
      AND w.candidate = reviewed_head;
    IF NOT FOUND OR NOT EXISTS (
        SELECT 1 FROM craft_queue.publications AS p
        WHERE p.work_id = order_id AND p.generation = source_generation AND p.head = reviewed_head
          AND p.state = 'confirmed' AND p.result_url = draft_url
    ) THEN RETURN NULL; END IF;
    IF EXISTS (
        SELECT 1 FROM craft_queue.publications AS p WHERE p.work_id = order_id
          AND p.state IN ('reserved', 'sending','uncertain')
    ) THEN RETURN NULL; END IF;

    INSERT INTO craft_queue.repair_requests (
        work_id, from_generation, head, native_url, actor, evidence, note
    ) VALUES (order_id, source_generation, reviewed_head, draft_url, actor_id, evidence_ref, feedback)
    RETURNING * INTO previous;
    UPDATE craft_queue.work AS w SET state = 'queued', repair_from_generation = source_generation,
        candidate = NULL, candidate_evidence = NULL WHERE w.id = order_id;
    RETURN to_jsonb(previous);
END;
$$;

CREATE FUNCTION craft_queue.repair_context(runner text, order_id text, current_generation bigint)
RETURNS jsonb
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT jsonb_build_object(
        'head', r.head, 'native_url', r.native_url, 'note', r.note, 'evidence', r.evidence,
        'digest', p.digest, 'pack_base64', replace(encode(p.payload, 'base64'), E'\n', '')
    )
    FROM craft_queue.work AS w JOIN craft_queue.repair_requests AS r
      ON r.work_id = w.id AND r.from_generation = w.repair_from_generation
    JOIN craft_queue.candidate_packs AS p ON p.work_id = r.work_id AND p.generation = r.from_generation
    JOIN craft_queue.attempts AS a ON a.work_id = w.id AND a.generation = w.generation
    JOIN craft_queue.runner_repositories AS allowed ON allowed.repository = w.repository
    WHERE w.id = order_id AND w.state = 'leased' AND w.generation = current_generation
      AND a.runner_id = runner AND allowed.runner_id = runner
      AND a.outcome = 'leased' AND a.expires_at > statement_timestamp();
$$;

CREATE OR REPLACE FUNCTION craft_queue.claim(runner text, request uuid)
RETURNS TABLE (work_id text, repository text, generation bigint, expires_at timestamptz, work_order jsonb)
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    settings craft_queue.control%ROWTYPE;
    selected craft_queue.work%ROWTYPE;
    previous craft_queue.attempts%ROWTYPE;
    now_at timestamptz;
    review_free boolean;
    tokens_free numeric;
    deadline timestamptz;
BEGIN
    SELECT * INTO STRICT settings FROM craft_queue.control FOR UPDATE;
    now_at := clock_timestamp();
    SELECT * INTO previous FROM craft_queue.attempts AS a
    WHERE a.runner_id = runner AND a.request_id = request;
    IF FOUND THEN
        RETURN QUERY SELECT w.id, w.repository, w.generation, previous.expires_at,
            (w.payload - 'repository') || CASE WHEN w.repair_from_generation IS NULL
                THEN '{}'::jsonb ELSE '{"shared_repair":true}'::jsonb END
        FROM craft_queue.work AS w
        JOIN craft_queue.runner_repositories AS grant_row ON grant_row.repository = w.repository
        WHERE w.id = previous.work_id AND w.generation = previous.generation
          AND w.state = 'leased' AND previous.outcome = 'leased' AND previous.expires_at > now_at
          AND grant_row.runner_id = runner;
        RETURN;
    END IF;
    IF settings.paused THEN RETURN; END IF;

    SELECT COUNT(*) < settings.review_limit INTO review_free
    FROM craft_queue.work AS w WHERE w.state IN ('leased', 'candidate')
      OR (w.state = 'queued' AND w.repair_from_generation IS NOT NULL);
    SELECT settings.token_budget - COALESCE(SUM(COALESCE(a.reported_tokens, a.token_limit)), 0)
    INTO tokens_free FROM craft_queue.attempts AS a
    WHERE a.started_at > now_at - interval '24 hours'
       OR (a.outcome = 'leased' AND a.expires_at > now_at);

    SELECT w.* INTO selected
    FROM craft_queue.work AS w
    JOIN craft_queue.runner_repositories AS grant_row ON grant_row.repository = w.repository
    LEFT JOIN craft_queue.attempts AS a ON a.work_id = w.id AND a.generation = w.generation
    WHERE grant_row.runner_id = runner AND w.reserved_tokens <= tokens_free
      AND ((w.state = 'queued' AND (review_free OR w.repair_from_generation IS NOT NULL))
        OR (w.state = 'leased' AND a.outcome = 'leased' AND a.expires_at <= now_at))
    ORDER BY (w.state = 'leased' OR w.repair_from_generation IS NOT NULL) DESC, w.created_at, w.id
    LIMIT 1;
    IF NOT FOUND THEN RETURN; END IF;

    UPDATE craft_queue.attempts AS a SET outcome = 'expired'
    WHERE a.work_id = selected.id AND a.generation = selected.generation AND a.outcome = 'leased';
    UPDATE craft_queue.work AS w SET state = 'leased', generation = w.generation + 1
    WHERE w.id = selected.id RETURNING w.* INTO selected;
    deadline := now_at + make_interval(secs => settings.lease_seconds);
    INSERT INTO craft_queue.attempts (work_id, generation, runner_id, request_id, expires_at, started_at, token_limit)
    VALUES (selected.id, selected.generation, runner, request, deadline, now_at, selected.reserved_tokens);
    RETURN QUERY SELECT selected.id, selected.repository, selected.generation, deadline,
        (selected.payload - 'repository') || CASE WHEN selected.repair_from_generation IS NULL
            THEN '{}'::jsonb ELSE '{"shared_repair":true}'::jsonb END;
END;
$$;

REVOKE ALL ON FUNCTION craft_queue.request_repair(text, text, bigint, text, text, text, text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.repair_context(text, text, bigint) FROM PUBLIC;

CREATE OR REPLACE FUNCTION craft_queue.candidate_pack(order_id text) RETURNS jsonb
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT jsonb_build_object(
        'work_id', w.id, 'repository', w.repository, 'generation', w.generation,
        'commit', w.candidate, 'digest', p.digest,
        'work_order', w.payload - 'repository',
        'pack_base64', replace(encode(p.payload, 'base64'), E'\n', '')
    ) || CASE WHEN feedback.head IS NULL THEN '{}'::jsonb
              ELSE jsonb_build_object('repair_head', feedback.head) END
    FROM craft_queue.work AS w JOIN craft_queue.candidate_packs AS p
      ON p.work_id = w.id AND p.generation = w.generation
    LEFT JOIN craft_queue.repair_requests AS feedback
      ON feedback.work_id = w.id AND feedback.from_generation = w.repair_from_generation
    WHERE w.id = order_id AND w.state = 'candidate';
$$;

ALTER FUNCTION craft_queue.operation_summary() RENAME TO operation_summary_before_repairs;
CREATE FUNCTION craft_queue.operation_summary() RETURNS jsonb
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT sample.summary || jsonb_build_object(
        'queued_repairs', repairs.total,
        'review_reserved', (sample.summary->>'review_reserved')::bigint + repairs.total
    )
    FROM (SELECT craft_queue.operation_summary_before_repairs() AS summary) AS sample
    CROSS JOIN (
        SELECT count(*) AS total FROM craft_queue.work AS w
        WHERE w.state = 'queued' AND w.repair_from_generation IS NOT NULL
    ) AS repairs;
$$;
REVOKE ALL ON FUNCTION craft_queue.operation_summary() FROM PUBLIC;

ALTER FUNCTION craft_queue.reserve_draft(text, text, bigint, text, bigint)
    RENAME TO reserve_new_draft;
CREATE FUNCTION craft_queue.reserve_draft(
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
              WHERE p.work_id = w.id AND p.generation = current_generation
          )
    ) THEN RETURN NULL; END IF;
    RETURN craft_queue.reserve_new_draft(actor, order_id, current_generation, candidate_head, review_sequence);
END;
$$;
REVOKE ALL ON FUNCTION craft_queue.reserve_draft(text, text, bigint, text, bigint) FROM PUBLIC;
COMMIT;
