BEGIN;

CREATE TABLE craft_queue.verifications (
    sequence bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    work_id text NOT NULL,
    generation bigint NOT NULL,
    head text NOT NULL,
    digest text NOT NULL,
    verifier text NOT NULL,
    passed boolean NOT NULL,
    evidence text NOT NULL,
    note text NOT NULL,
    observed_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    UNIQUE (work_id, generation, evidence),
    FOREIGN KEY (work_id, generation) REFERENCES craft_queue.candidate_packs(work_id, generation)
);

CREATE INDEX verifications_current ON craft_queue.verifications (work_id, generation, sequence DESC);

CREATE FUNCTION craft_queue.record_verification(
    actor text, order_id text, candidate_generation bigint, candidate_head text, pack_digest text,
    previous_sequence bigint, decision_passed boolean, evidence_ref text, decision_note text
) RETURNS bigint
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    existing craft_queue.verifications%ROWTYPE;
    latest bigint;
    recorded bigint;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    IF COALESCE(actor, '') !~ '^[A-Za-z0-9_-]{1,100}$'
        OR COALESCE(candidate_head, '') !~ '^([a-f0-9]{40}|[a-f0-9]{64})$'
        OR COALESCE(pack_digest, '') !~ '^sha256:[a-f0-9]{64}$'
        OR previous_sequence IS NULL OR previous_sequence < 0
        OR decision_passed IS NULL
        OR length(btrim(COALESCE(evidence_ref, ''))) NOT BETWEEN 1 AND 500
        OR length(btrim(COALESCE(decision_note, ''))) NOT BETWEEN 1 AND 1000 THEN
        RAISE EXCEPTION 'invalid verification record';
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM craft_queue.work AS w
        JOIN craft_queue.attempts AS a ON a.work_id = w.id AND a.generation = w.generation
        JOIN craft_queue.candidate_packs AS p ON p.work_id = w.id AND p.generation = w.generation
        WHERE w.id = order_id AND w.state = 'candidate' AND w.generation = candidate_generation
          AND w.candidate = candidate_head AND w.candidate_evidence = pack_digest
          AND p.digest = pack_digest AND a.runner_id <> actor
    ) THEN
        RETURN NULL;
    END IF;

    SELECT * INTO existing FROM craft_queue.verifications AS v
    WHERE v.work_id = order_id AND v.generation = candidate_generation AND v.evidence = evidence_ref;
    IF FOUND THEN
        IF existing.verifier = actor AND existing.head = candidate_head AND existing.digest = pack_digest
            AND existing.passed = decision_passed AND existing.note = decision_note THEN
            RETURN existing.sequence;
        END IF;
        RETURN NULL;
    END IF;
    SELECT COALESCE(MAX(v.sequence), 0) INTO latest FROM craft_queue.verifications AS v
    WHERE v.work_id = order_id AND v.generation = candidate_generation;
    IF latest <> previous_sequence THEN RETURN NULL; END IF;

    INSERT INTO craft_queue.verifications (work_id, generation, head, digest, verifier, passed, evidence, note)
    VALUES (order_id, candidate_generation, candidate_head, pack_digest, actor,
            decision_passed, evidence_ref, decision_note)
    RETURNING sequence INTO recorded;
    RETURN recorded;
END;
$$;

CREATE OR REPLACE FUNCTION craft_queue.status(work_id text) RETURNS jsonb
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT jsonb_build_object(
        'id', w.id, 'repository', w.repository, 'state', w.state,
        'generation', w.generation, 'candidate', w.candidate,
        'candidate_evidence', w.candidate_evidence, 'cancellation_reason', w.cancellation_reason,
        'verification', to_jsonb(latest),
        'independent_checks_passed', w.state = 'candidate' AND COALESCE(latest.passed, false)
    )
    FROM craft_queue.work AS w
    LEFT JOIN LATERAL (
        SELECT v.sequence, v.verifier, v.passed, v.evidence, v.note, v.observed_at
        FROM craft_queue.verifications AS v
        WHERE v.work_id = w.id AND v.generation = w.generation
          AND v.head = w.candidate AND v.digest = w.candidate_evidence
        ORDER BY v.sequence DESC LIMIT 1
    ) AS latest ON true
    WHERE w.id = work_id;
$$;

REVOKE ALL ON FUNCTION craft_queue.record_verification(
    text, text, bigint, text, text, bigint, boolean, text, text
) FROM PUBLIC;
COMMIT;
