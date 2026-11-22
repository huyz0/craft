BEGIN;

CREATE OR REPLACE FUNCTION craft_queue.record_review_decision_with_repair(
    reviewer text, handoff_id text, candidate_head text, verdict text, evidence_ref text, feedback text
) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    already_decided boolean;
    recorded_decision jsonb;
    authorized_handoff craft_queue.review_handoffs%ROWTYPE;
    repair jsonb;
    publication_ref text;
    verified_sequence bigint;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    SELECT EXISTS (SELECT 1 FROM craft_queue.review_handoffs AS h WHERE h.id = handoff_id
                   AND h.decision IS NOT NULL) INTO already_decided;
    recorded_decision := craft_queue.record_review_decision(
        reviewer, handoff_id, candidate_head, verdict, evidence_ref, feedback);
    IF recorded_decision IS NULL OR already_decided THEN RETURN recorded_decision; END IF;
    publication_ref := recorded_decision->>'publication_key';
    verified_sequence := (recorded_decision->>'verification_sequence')::bigint;

    IF EXISTS (
        SELECT 1 FROM craft_queue.review_handoffs AS h
        JOIN craft_queue.review_inbox_grants AS g ON g.grant_id = h.grant_id
        WHERE h.publication_key = publication_ref AND h.verification_sequence = verified_sequence
          AND h.decision IS NULL AND g.revoked_at IS NULL AND g.expires_at > clock_timestamp()
    ) THEN
        IF verdict <> 'changes_requested' OR NOT (recorded_decision->>'queue_repair')::boolean
        THEN RETURN recorded_decision; END IF;
    ELSE
        SELECT h.* INTO authorized_handoff FROM craft_queue.review_handoffs AS h
        JOIN craft_queue.review_inbox_grants AS g ON g.grant_id = h.grant_id
        WHERE h.publication_key = publication_ref AND h.verification_sequence = verified_sequence
          AND h.decision = 'changes_requested' AND h.queue_repair
          AND g.revoked_at IS NULL AND g.expires_at > clock_timestamp()
        ORDER BY (h.id = handoff_id) DESC, h.decided_at, h.id
        LIMIT 1;
        IF NOT FOUND THEN RETURN recorded_decision; END IF;
        repair := craft_queue.apply_review_repair(authorized_handoff.requested_by, authorized_handoff.id);
    END IF;

    UPDATE craft_queue.review_handoffs AS h
    SET automatic_repair = CASE WHEN repair IS NULL THEN 'needs_operator' ELSE 'queued' END
    WHERE h.id = handoff_id RETURNING to_jsonb(h) INTO recorded_decision;
    RETURN recorded_decision;
END;
$$;

COMMIT;
