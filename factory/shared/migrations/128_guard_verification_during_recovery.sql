BEGIN;

CREATE OR REPLACE FUNCTION craft_queue.verification_pickup_allows_verdict(
    pickup_id uuid, actor text, order_id text, candidate_generation bigint, evidence_ref text
) RETURNS boolean LANGUAGE plpgsql SET search_path = pg_catalog AS $$
DECLARE
    picked craft_queue.verification_pickups%ROWTYPE;
BEGIN
    -- Both verdict writers hold the queue control lock used to start and finish recovery.
    IF EXISTS (SELECT 1 FROM craft_queue.recoveries WHERE finished_at IS NULL) THEN RETURN false; END IF;
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

COMMIT;
