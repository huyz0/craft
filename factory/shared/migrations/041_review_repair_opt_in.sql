BEGIN;

ALTER TABLE craft_queue.review_handoffs
    ADD COLUMN queue_repair boolean NOT NULL DEFAULT false,
    ADD COLUMN automatic_repair text CHECK (automatic_repair IN ('queued', 'needs_operator'));

CREATE FUNCTION craft_queue.assign_review_with_repair(
    actor text, handoff_id text, order_id text, candidate_generation bigint,
    candidate_head text, review_sequence bigint, reviewer text, evidence_ref text, queue_repair boolean
) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    retained boolean;
    assigned jsonb;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    SELECT h.queue_repair INTO retained FROM craft_queue.review_handoffs AS h WHERE h.id = handoff_id;
    IF FOUND AND retained IS DISTINCT FROM queue_repair THEN RETURN NULL; END IF;
    assigned := craft_queue.assign_review(actor, handoff_id, order_id, candidate_generation,
        candidate_head, review_sequence, reviewer, evidence_ref);
    IF assigned IS NULL OR NOT queue_repair THEN RETURN assigned; END IF;
    UPDATE craft_queue.review_handoffs AS h SET queue_repair = true WHERE h.id = handoff_id
    RETURNING to_jsonb(h) INTO assigned;
    RETURN assigned;
END;
$$;

CREATE FUNCTION craft_queue.record_review_decision_with_repair(
    reviewer text, handoff_id text, candidate_head text, verdict text, evidence_ref text, feedback text
) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    decision jsonb;
    repair jsonb;
BEGIN
    decision := craft_queue.record_review_decision(reviewer, handoff_id, candidate_head, verdict, evidence_ref, feedback);
    IF decision IS NULL OR verdict <> 'changes_requested'
       OR NOT (decision->>'queue_repair')::boolean OR decision->>'automatic_repair' IS NOT NULL THEN
        RETURN decision;
    END IF;
    IF EXISTS (
        SELECT 1 FROM craft_queue.work AS w CROSS JOIN craft_queue.control AS settings
        WHERE w.id = decision->>'work_id' AND NOT settings.paused
          AND craft_queue.standing_delegation_current(w.id, clock_timestamp())
          AND (w.payload->>'execution_profile' IS NULL
              OR w.generation < (w.payload->'execution_profile'->>'max_attempts')::bigint)
    ) THEN
        repair := craft_queue.apply_review_repair(decision->>'requested_by', handoff_id);
    END IF;
    UPDATE craft_queue.review_handoffs AS h
    SET automatic_repair = CASE WHEN repair IS NULL THEN 'needs_operator' ELSE 'queued' END
    WHERE h.id = handoff_id RETURNING to_jsonb(h) INTO decision;
    RETURN decision;
END;
$$;

REVOKE ALL ON FUNCTION craft_queue.assign_review_with_repair(text, text, text, bigint, text, bigint, text, text, boolean) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.record_review_decision_with_repair(text, text, text, text, text, text) FROM PUBLIC;

CREATE OR REPLACE FUNCTION craft_queue.pending_review_handoffs(reviewer text, repo text, owner_name text, maximum integer)
RETURNS jsonb LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT COALESCE(jsonb_agg(to_jsonb(pending) ORDER BY requested_at, id), '[]'::jsonb)
    FROM (
        SELECT h.id, h.work_id, w.repository, h.generation, h.head, h.request_evidence,
               h.requested_at, h.queue_repair, p.result_url AS draft_url
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

COMMIT;
