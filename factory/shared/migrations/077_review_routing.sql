BEGIN;

ALTER TABLE craft_queue.review_handoffs ADD COLUMN routing jsonb;

CREATE FUNCTION craft_queue.route_review(
    actor text, assignment_id text, order_id text, candidate_generation bigint, candidate_head text,
    review_sequence bigint, roster text[], max_pending integer, evidence_ref text, queue_repair boolean
) RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    handoff craft_queue.review_handoffs%ROWTYPE;
    chosen text;
    pending bigint;
    policy jsonb;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    IF roster IS NULL OR cardinality(roster) NOT BETWEEN 1 AND 25 OR max_pending IS NULL
        OR max_pending NOT BETWEEN 1 AND 50 OR queue_repair IS NULL
        OR (SELECT count(DISTINCT member) FROM unnest(roster) AS entry(member)) <> cardinality(roster)
        THEN RETURN NULL; END IF;
    policy := jsonb_build_object('roster', roster, 'max_pending', max_pending);
    SELECT h.* INTO handoff FROM craft_queue.review_handoffs AS h WHERE h.id = assignment_id;
    IF FOUND THEN
        IF handoff.requested_by IS DISTINCT FROM actor OR handoff.work_id IS DISTINCT FROM order_id
            OR handoff.generation IS DISTINCT FROM candidate_generation OR handoff.head IS DISTINCT FROM candidate_head
            OR handoff.verification_sequence IS DISTINCT FROM review_sequence
            OR handoff.request_evidence IS DISTINCT FROM evidence_ref OR handoff.queue_repair IS DISTINCT FROM queue_repair
            OR (handoff.routing - 'pending_before') IS DISTINCT FROM policy THEN RETURN NULL; END IF;
    ELSE
        IF craft_queue.current_review_draft(order_id, candidate_generation, candidate_head, review_sequence)
            IS NULL THEN RETURN NULL; END IF;

        WITH load AS (
            SELECT h.reviewer_id, count(*) AS pending
            FROM craft_queue.current_review_waits AS h
            WHERE h.reviewer_id = ANY(roster)
            GROUP BY h.reviewer_id
        )
        SELECT c.reviewer_id, COALESCE(load.pending, 0) INTO chosen, pending
        FROM unnest(roster) WITH ORDINALITY AS c(reviewer_id, position)
        JOIN craft_queue.work AS w ON w.id = order_id
        JOIN craft_queue.attempts AS a ON a.work_id = w.id AND a.generation = w.generation
        LEFT JOIN load ON load.reviewer_id = c.reviewer_id
        WHERE c.reviewer_id <> a.runner_id AND COALESCE(load.pending, 0) < max_pending
          AND EXISTS (SELECT 1 FROM craft_queue.review_inbox_grants AS g
              WHERE g.reviewer_id = c.reviewer_id AND g.repository = w.repository
                AND g.revoked_at IS NULL AND g.expires_at > statement_timestamp())
          AND NOT EXISTS (SELECT 1 FROM craft_queue.review_handoffs AS h
              WHERE h.work_id = order_id AND h.generation = candidate_generation
                AND h.verification_sequence = review_sequence AND h.reviewer_id = c.reviewer_id
                AND h.decision IS DISTINCT FROM 'withdrawn')
        ORDER BY COALESCE(load.pending, 0), c.position LIMIT 1;

        IF NOT FOUND OR craft_queue.assign_review_with_repair(actor, assignment_id, order_id,
            candidate_generation, candidate_head, review_sequence, chosen, evidence_ref, queue_repair)
            IS NULL THEN RETURN NULL; END IF;
        UPDATE craft_queue.review_handoffs AS h SET routing = policy || jsonb_build_object('pending_before', pending)
        WHERE h.id = assignment_id RETURNING * INTO handoff;
    END IF;

    RETURN jsonb_build_object(
        'handoff_id', handoff.id, 'reviewer_id', handoff.reviewer_id, 'work_id', handoff.work_id,
        'head', handoff.head, 'generation', handoff.generation, 'verification_sequence', handoff.verification_sequence,
        'queue_repair', handoff.queue_repair, 'requested_at', handoff.requested_at, 'routing', handoff.routing
    );
END;
$$;

REVOKE ALL ON FUNCTION craft_queue.route_review(text, text, text, bigint, text, bigint, text[], integer, text, boolean) FROM PUBLIC;

COMMIT;
