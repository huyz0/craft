BEGIN;

ALTER TABLE craft_queue.repair_requests ADD COLUMN review_handoff_ids text[];

CREATE OR REPLACE FUNCTION craft_queue.apply_review_repair(actor text, handoff_id text) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    handoff craft_queue.review_handoffs%ROWTYPE;
    previous craft_queue.repair_requests%ROWTYPE;
    draft_url text;
    included_ids text[];
    bundled_note text;
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
           AND (handoff_id = ANY(previous.review_handoff_ids)
                OR (previous.review_handoff_ids IS NULL AND previous.evidence = handoff.decision_evidence
                    AND previous.note = handoff.decision_note))
        THEN RETURN to_jsonb(previous); END IF;
        RETURN NULL;
    END IF;
    IF handoff.publication_key IS DISTINCT FROM craft_queue.current_review_draft(
        handoff.work_id, handoff.generation, handoff.head, handoff.verification_sequence
    ) OR NOT EXISTS (
        SELECT 1 FROM craft_queue.work AS w CROSS JOIN craft_queue.control AS settings
        WHERE w.id = handoff.work_id AND NOT settings.paused
          AND craft_queue.standing_delegation_current(w.id, clock_timestamp())
          AND (w.payload->>'execution_profile' IS NULL
              OR w.generation < (w.payload->'execution_profile'->>'max_attempts')::bigint)
    ) THEN RETURN NULL; END IF;

    SELECT array_agg(h.id ORDER BY h.decided_at, h.id),
        string_agg(format(E'Review %s (%s)\n%s', h.id, h.decision_evidence, h.decision_note),
                   E'\n\n' ORDER BY h.decided_at, h.id)
    INTO included_ids, bundled_note
    FROM craft_queue.review_handoffs AS h
    WHERE h.work_id = handoff.work_id AND h.generation = handoff.generation AND h.head = handoff.head
      AND h.verification_sequence = handoff.verification_sequence AND h.publication_key = handoff.publication_key
      AND h.decision = 'changes_requested';
    IF cardinality(included_ids) = 1 THEN bundled_note := handoff.decision_note; END IF;
    IF bundled_note IS NULL OR length(bundled_note) > 4000 THEN RETURN NULL; END IF;

    IF craft_queue.request_repair(actor, handoff.work_id, handoff.generation, handoff.head, draft_url,
        handoff.decision_evidence, bundled_note) IS NULL THEN RETURN NULL; END IF;
    UPDATE craft_queue.repair_requests AS r SET review_handoff_ids = included_ids
    WHERE r.work_id = handoff.work_id AND r.from_generation = handoff.generation
    RETURNING r.* INTO previous;
    RETURN to_jsonb(previous);
END;
$$;

CREATE OR REPLACE FUNCTION craft_queue.record_review_decision_with_repair(
    reviewer text, handoff_id text, candidate_head text, verdict text, evidence_ref text, feedback text
) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    recorded_decision jsonb;
    repair jsonb;
BEGIN
    recorded_decision := craft_queue.record_review_decision(reviewer, handoff_id, candidate_head, verdict, evidence_ref, feedback);
    IF recorded_decision IS NULL OR verdict <> 'changes_requested'
       OR NOT (recorded_decision->>'queue_repair')::boolean OR recorded_decision->>'automatic_repair' IS NOT NULL THEN
        RETURN recorded_decision;
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM craft_queue.review_handoffs AS h
        JOIN craft_queue.review_inbox_grants AS g ON g.grant_id = h.grant_id
        WHERE h.publication_key = recorded_decision->>'publication_key'
          AND h.verification_sequence = (recorded_decision->>'verification_sequence')::bigint AND h.decision IS NULL
          AND g.revoked_at IS NULL AND g.expires_at > clock_timestamp()
    ) THEN
        repair := craft_queue.apply_review_repair(recorded_decision->>'requested_by', handoff_id);
    END IF;
    UPDATE craft_queue.review_handoffs AS h
    SET automatic_repair = CASE WHEN repair IS NULL THEN 'needs_operator' ELSE 'queued' END
    WHERE h.id = handoff_id RETURNING to_jsonb(h) INTO recorded_decision;
    RETURN recorded_decision;
END;
$$;

COMMIT;
