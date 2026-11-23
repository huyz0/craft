BEGIN;

ALTER TABLE craft_queue.review_handoffs
    DROP CONSTRAINT review_handoffs_decision_check,
    DROP CONSTRAINT review_handoffs_work_id_generation_verification_sequence_gr_key,
    ADD COLUMN withdrawn_by text,
    ADD COLUMN replacement_handoff_id text REFERENCES craft_queue.review_handoffs(id),
    ADD CONSTRAINT review_handoffs_decision_check CHECK (
        decision IN ('acknowledged', 'changes_requested', 'withdrawn')
    ),
    ADD CONSTRAINT review_handoffs_withdrawal_binding CHECK (
        (withdrawn_by IS NOT NULL) = (decision IS NOT DISTINCT FROM 'withdrawn')
        AND (replacement_handoff_id IS NOT NULL) = (decision IS NOT DISTINCT FROM 'withdrawn')
    );

CREATE UNIQUE INDEX review_handoffs_revision_grant
ON craft_queue.review_handoffs (work_id, generation, verification_sequence, grant_id)
WHERE decision IS DISTINCT FROM 'withdrawn';

CREATE FUNCTION craft_queue.reassign_review(
    actor text, previous_id text, replacement_id text, candidate_head text,
    next_reviewer text, evidence_ref text, allow_automatic_repair boolean
) RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    previous craft_queue.review_handoffs%ROWTYPE;
    assigned jsonb;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    SELECT h.* INTO previous FROM craft_queue.review_handoffs AS h WHERE h.id = previous_id;
    IF NOT FOUND OR previous.head IS DISTINCT FROM candidate_head
        OR previous.reviewer_id = next_reviewer OR previous_id = replacement_id THEN RETURN NULL; END IF;

    IF previous.decision = 'withdrawn' THEN
        IF previous.withdrawn_by IS DISTINCT FROM actor OR previous.replacement_handoff_id IS DISTINCT FROM replacement_id
            OR previous.decision_evidence IS DISTINCT FROM evidence_ref
            OR NOT EXISTS (SELECT 1 FROM craft_queue.review_handoffs AS h WHERE h.id = replacement_id
                AND h.reviewer_id = next_reviewer AND h.queue_repair = allow_automatic_repair)
            THEN RETURN NULL; END IF;
    ELSE
        IF previous.decision IS NOT NULL OR EXISTS (
            SELECT 1 FROM craft_queue.review_handoffs AS h WHERE h.id = replacement_id
        ) OR previous.publication_key IS DISTINCT FROM craft_queue.current_review_draft(
            previous.work_id, previous.generation, previous.head, previous.verification_sequence
        ) THEN RETURN NULL; END IF;

        assigned := craft_queue.assign_review_with_repair(actor, replacement_id, previous.work_id,
            previous.generation, previous.head, previous.verification_sequence, next_reviewer,
            evidence_ref, allow_automatic_repair);
        IF assigned IS NULL THEN RETURN NULL; END IF;

        UPDATE craft_queue.review_handoffs AS h SET decision = 'withdrawn', decision_evidence = evidence_ref,
            decided_at = clock_timestamp(), withdrawn_by = actor, replacement_handoff_id = replacement_id
        WHERE h.id = previous_id RETURNING * INTO previous;
    END IF;
    RETURN jsonb_build_object(
        'previous_handoff_id', previous.id, 'replacement_handoff_id', replacement_id,
        'work_id', previous.work_id, 'head', previous.head, 'generation', previous.generation,
        'verification_sequence', previous.verification_sequence, 'reviewer_id', next_reviewer,
        'queue_repair', allow_automatic_repair, 'actor', actor,
        'evidence', previous.decision_evidence, 'reassigned_at', previous.decided_at
    );
END;
$$;

REVOKE ALL ON FUNCTION craft_queue.reassign_review(text, text, text, text, text, text, boolean) FROM PUBLIC;

COMMIT;
