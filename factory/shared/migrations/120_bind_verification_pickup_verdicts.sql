BEGIN;

ALTER TABLE craft_queue.verification_pickups ADD COLUMN verification_sequence bigint
    REFERENCES craft_queue.verifications(sequence) ON DELETE SET NULL;

CREATE FUNCTION craft_queue.verification_pickup_allows_verdict(
    pickup_id uuid, actor text, order_id text, candidate_generation bigint, evidence_ref text
) RETURNS boolean LANGUAGE plpgsql SET search_path = pg_catalog AS $$
DECLARE
    picked craft_queue.verification_pickups%ROWTYPE;
BEGIN
    IF pickup_id IS NULL THEN RETURN true; END IF;
    SELECT * INTO picked FROM craft_queue.verification_pickups AS pickup
    WHERE pickup.id = pickup_id AND pickup.verifier = actor
      AND pickup.work_id = order_id AND pickup.generation = candidate_generation
    FOR UPDATE;
    IF NOT FOUND THEN RETURN false; END IF;
    IF picked.verification_sequence IS NOT NULL THEN
        RETURN EXISTS (SELECT 1 FROM craft_queue.verifications AS v
            WHERE v.sequence = picked.verification_sequence AND v.evidence = evidence_ref);
    END IF;
    RETURN picked.released_at IS NULL AND picked.expires_at > clock_timestamp()
        AND NOT EXISTS (SELECT 1 FROM craft_queue.recoveries AS recovery WHERE recovery.started_at >= picked.picked_at);
END;
$$;

CREATE FUNCTION craft_queue.record_picked_verification(
    actor text, order_id text, candidate_generation bigint, candidate_head text, pack_digest text,
    previous_sequence bigint, decision_passed boolean, evidence_ref text, decision_note text, summary text, pickup_id uuid
) RETURNS bigint LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    recorded bigint;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    IF NOT craft_queue.verification_pickup_allows_verdict(pickup_id, actor, order_id, candidate_generation, evidence_ref)
        THEN RETURN NULL; END IF;
    recorded := craft_queue.record_reviewable_verification(actor, order_id, candidate_generation, candidate_head,
        pack_digest, previous_sequence, decision_passed, evidence_ref, decision_note, summary);
    IF recorded IS NOT NULL THEN
        UPDATE craft_queue.verification_pickups SET verification_sequence = recorded WHERE id = pickup_id;
    END IF;
    RETURN recorded;
END;
$$;

CREATE FUNCTION craft_queue.record_picked_dependency_verification(
    actor text, order_id text, candidate_generation bigint, candidate_head text, pack_digest text,
    previous_sequence bigint, decision_passed boolean, approval_id text, proof_data jsonb, pickup_id uuid
) RETURNS bigint LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    recorded bigint;
    evidence_ref text := 'dependency-compatibility:' || approval_id || ':' || (proof_data->>'report_digest');
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    IF NOT craft_queue.verification_pickup_allows_verdict(pickup_id, actor, order_id, candidate_generation, evidence_ref)
        THEN RETURN NULL; END IF;
    recorded := craft_queue.record_dependency_verification(actor, order_id, candidate_generation, candidate_head,
        pack_digest, previous_sequence, decision_passed, approval_id, proof_data);
    IF recorded IS NOT NULL THEN
        UPDATE craft_queue.verification_pickups SET verification_sequence = recorded WHERE id = pickup_id;
    END IF;
    RETURN recorded;
END;
$$;

REVOKE ALL ON FUNCTION craft_queue.verification_pickup_allows_verdict(uuid,text,text,bigint,text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.record_picked_verification(text,text,bigint,text,text,bigint,boolean,text,text,text,uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.record_picked_dependency_verification(text,text,bigint,text,text,bigint,boolean,text,jsonb,uuid) FROM PUBLIC;

COMMIT;
