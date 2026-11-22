BEGIN;

CREATE TABLE craft_queue.review_handoffs (
    id text PRIMARY KEY,
    work_id text NOT NULL,
    generation bigint NOT NULL,
    head text NOT NULL,
    verification_sequence bigint NOT NULL REFERENCES craft_queue.verifications(sequence),
    publication_key text NOT NULL REFERENCES craft_queue.publications(action_key),
    reviewer_id text NOT NULL,
    grant_id text NOT NULL REFERENCES craft_queue.review_inbox_grants(grant_id),
    requested_by text NOT NULL,
    request_evidence text NOT NULL,
    requested_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    decision text CHECK (decision IN ('acknowledged','changes_requested')),
    decision_evidence text,
    decision_note text,
    decided_at timestamptz,
    UNIQUE (work_id, generation, verification_sequence, grant_id),
    FOREIGN KEY (work_id, generation) REFERENCES craft_queue.candidate_packs(work_id, generation)
);
CREATE INDEX review_handoffs_pending ON craft_queue.review_handoffs (reviewer_id, requested_at) WHERE decision IS NULL;

CREATE FUNCTION craft_queue.current_review_draft(
    order_id text, candidate_generation bigint, candidate_head text, review_sequence bigint
) RETURNS text
LANGUAGE sql STABLE SET search_path = pg_catalog AS $$
    SELECT p.action_key FROM craft_queue.work AS w
    JOIN craft_queue.publications AS p ON p.work_id = w.id AND p.generation = w.generation
        AND p.head = w.candidate AND p.pack_digest = w.candidate_evidence
    JOIN LATERAL (
        SELECT v.* FROM craft_queue.verifications AS v WHERE v.work_id = w.id AND v.generation = w.generation
        ORDER BY v.sequence DESC LIMIT 1
    ) AS v ON v.sequence = review_sequence AND v.passed
        AND v.head = w.candidate AND v.digest = w.candidate_evidence
    WHERE w.id = order_id AND w.state = 'candidate' AND w.generation = candidate_generation
      AND w.candidate = candidate_head AND p.state = 'confirmed' AND p.result_url IS NOT NULL
      AND craft_queue.series_blocker(w.id) IS NULL;
$$;

CREATE FUNCTION craft_queue.assign_review(
    actor text, handoff_id text, order_id text, candidate_generation bigint,
    candidate_head text, review_sequence bigint, reviewer text, evidence_ref text
) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    handoff craft_queue.review_handoffs%ROWTYPE;
    scope_id text;
    draft_key text;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    IF length(btrim(COALESCE(evidence_ref, ''))) NOT BETWEEN 1 AND 500 THEN RETURN NULL; END IF;
    SELECT * INTO handoff FROM craft_queue.review_handoffs AS h WHERE h.id = handoff_id;
    IF FOUND THEN
        IF handoff.requested_by = actor AND handoff.work_id = order_id
            AND handoff.generation = candidate_generation AND handoff.head = candidate_head
            AND handoff.verification_sequence = review_sequence AND handoff.reviewer_id = reviewer
            AND handoff.request_evidence = evidence_ref THEN RETURN to_jsonb(handoff); END IF;
        RETURN NULL;
    END IF;
    SELECT g.grant_id INTO scope_id FROM craft_queue.review_inbox_grants AS g
    JOIN craft_queue.work AS w ON w.repository = g.repository
    JOIN craft_queue.attempts AS a ON a.work_id = w.id AND a.generation = w.generation
    WHERE w.id = order_id AND g.reviewer_id = reviewer AND a.runner_id <> reviewer
        AND g.revoked_at IS NULL AND g.expires_at > clock_timestamp()
    ORDER BY g.granted_at DESC LIMIT 1;
    draft_key := craft_queue.current_review_draft(order_id, candidate_generation, candidate_head, review_sequence);
    IF scope_id IS NULL OR draft_key IS NULL THEN RETURN NULL; END IF;
    INSERT INTO craft_queue.review_handoffs (id, work_id, generation, head, verification_sequence,
        publication_key, reviewer_id, grant_id, requested_by, request_evidence)
    VALUES (handoff_id, order_id, candidate_generation, candidate_head, review_sequence, draft_key,
        reviewer, scope_id, actor, evidence_ref)
    ON CONFLICT DO NOTHING RETURNING * INTO handoff;
    IF NOT FOUND THEN RETURN NULL; END IF;
    RETURN to_jsonb(handoff);
END;
$$;

CREATE FUNCTION craft_queue.record_review_decision(
    reviewer text, handoff_id text, candidate_head text, verdict text, evidence_ref text, feedback text
) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    handoff craft_queue.review_handoffs%ROWTYPE;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    IF COALESCE(verdict, '') NOT IN ('acknowledged', 'changes_requested')
       OR length(btrim(COALESCE(evidence_ref, ''))) NOT BETWEEN 1 AND 500
       OR length(btrim(COALESCE(feedback, ''))) NOT BETWEEN 1 AND 1000 THEN RETURN NULL; END IF;
    SELECT h.* INTO handoff FROM craft_queue.review_handoffs AS h
    JOIN craft_queue.review_inbox_grants AS g ON g.grant_id = h.grant_id
    WHERE h.id = handoff_id AND h.reviewer_id = reviewer AND h.head = candidate_head
      AND g.revoked_at IS NULL AND g.expires_at > clock_timestamp();
    IF NOT FOUND THEN RETURN NULL; END IF;
    IF handoff.decision IS NOT NULL THEN
        IF handoff.decision = verdict AND handoff.decision_evidence = evidence_ref
            AND handoff.decision_note = feedback THEN RETURN to_jsonb(handoff); END IF;
        RETURN NULL;
    END IF;
    IF handoff.publication_key IS DISTINCT FROM craft_queue.current_review_draft(
        handoff.work_id, handoff.generation, handoff.head, handoff.verification_sequence
    ) THEN RETURN NULL; END IF;
    UPDATE craft_queue.review_handoffs AS h
    SET decision = verdict, decision_evidence = evidence_ref, decision_note = feedback, decided_at = clock_timestamp()
    WHERE h.id = handoff_id RETURNING * INTO handoff;
    RETURN to_jsonb(handoff);
END;
$$;

CREATE FUNCTION craft_queue.apply_review_repair(actor text, handoff_id text) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    handoff craft_queue.review_handoffs%ROWTYPE;
    previous craft_queue.repair_requests%ROWTYPE;
    draft_url text;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    SELECT h.* INTO handoff FROM craft_queue.review_handoffs AS h
    WHERE h.id = handoff_id AND h.decision = 'changes_requested';
    IF NOT FOUND THEN RETURN NULL; END IF;
    SELECT p.result_url INTO draft_url FROM craft_queue.publications AS p WHERE p.action_key = handoff.publication_key;
    SELECT * INTO previous FROM craft_queue.repair_requests AS r
    WHERE r.work_id = handoff.work_id AND r.from_generation = handoff.generation;
    IF FOUND THEN
        IF previous.actor = actor AND previous.head = handoff.head AND previous.native_url = draft_url
            AND previous.evidence = handoff.decision_evidence AND previous.note = handoff.decision_note
        THEN RETURN to_jsonb(previous); END IF;
        RETURN NULL;
    END IF;
    IF handoff.publication_key IS DISTINCT FROM craft_queue.current_review_draft(
        handoff.work_id, handoff.generation, handoff.head, handoff.verification_sequence
    ) THEN RETURN NULL; END IF;
    RETURN craft_queue.request_repair(actor, handoff.work_id, handoff.generation, handoff.head, draft_url,
        handoff.decision_evidence, handoff.decision_note);
END;
$$;

CREATE FUNCTION craft_queue.pending_review_handoffs(reviewer text, repo text, owner_name text, maximum integer)
RETURNS jsonb LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT COALESCE(jsonb_agg(to_jsonb(pending) ORDER BY requested_at, id), '[]'::jsonb)
    FROM (
        SELECT h.id, h.work_id, w.repository, h.generation, h.head, h.request_evidence,
               h.requested_at, p.result_url AS draft_url
        FROM craft_queue.review_handoffs AS h
        JOIN craft_queue.review_inbox_grants AS g ON g.grant_id = h.grant_id
        JOIN craft_queue.work AS w ON w.id = h.work_id
        JOIN craft_queue.publications AS p ON p.action_key = h.publication_key
        WHERE h.reviewer_id = reviewer AND h.decision IS NULL
          AND g.revoked_at IS NULL AND g.expires_at > statement_timestamp()
          AND (repo IS NULL OR w.repository = repo) AND (owner_name IS NULL OR w.payload->>'owner' = owner_name)
          AND h.publication_key = craft_queue.current_review_draft(
              h.work_id, h.generation, h.head, h.verification_sequence
          )
        ORDER BY h.requested_at, h.id LIMIT least(greatest(maximum, 1), 50)
    ) AS pending;
$$;

CREATE FUNCTION craft_queue.review_handoffs_for(order_id text) RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT COALESCE(jsonb_agg(to_jsonb(receipt) ORDER BY receipt.requested_at DESC, receipt.id), '[]'::jsonb)
    FROM (SELECT h.* FROM craft_queue.review_handoffs AS h WHERE h.work_id = order_id
        ORDER BY h.requested_at DESC, h.id LIMIT 20) AS receipt;
$$;

REVOKE ALL ON FUNCTION craft_queue.current_review_draft(text, bigint, text, bigint) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.assign_review(text, text, text, bigint, text, bigint, text, text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.record_review_decision(text, text, text, text, text, text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.apply_review_repair(text, text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.pending_review_handoffs(text, text, text, integer) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.review_handoffs_for(text) FROM PUBLIC;
COMMIT;
