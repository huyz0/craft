BEGIN;

CREATE FUNCTION craft_queue.renew_verification_pickup(actor text, repo text, pickup_id uuid)
RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    renewed craft_queue.verification_pickups%ROWTYPE;
    now_at timestamptz;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    now_at := clock_timestamp();
    IF EXISTS (SELECT 1 FROM craft_queue.recoveries WHERE finished_at IS NULL) THEN RETURN NULL; END IF;

    UPDATE craft_queue.verification_pickups AS pickup
    SET expires_at = greatest(pickup.expires_at, least(now_at + interval '10 minutes', pickup.picked_at + interval '1 hour'))
    FROM craft_queue.work AS w
    WHERE pickup.id = pickup_id AND pickup.verifier = actor AND w.repository = repo
      AND w.id = pickup.work_id AND w.generation = pickup.generation AND w.state = 'candidate'
      AND pickup.released_at IS NULL AND pickup.expires_at > now_at
      AND NOT EXISTS (SELECT 1 FROM craft_queue.recoveries AS recovery WHERE recovery.started_at >= pickup.picked_at)
      AND craft_queue.standing_delegation_current(w.id, now_at)
      AND NOT EXISTS (SELECT 1 FROM craft_queue.verifications AS v
          WHERE v.work_id = w.id AND v.generation = w.generation AND v.head = w.candidate AND v.digest = w.candidate_evidence)
      AND NOT EXISTS (SELECT 1 FROM craft_queue.publications AS publication
          WHERE publication.work_id = w.id AND publication.state IN ('sending', 'uncertain'))
    RETURNING pickup.* INTO renewed;
    IF NOT FOUND THEN RETURN NULL; END IF;
    RETURN jsonb_build_object('pickup_id', renewed.id, 'pickup_expires_at', renewed.expires_at,
        'renewable_until', renewed.picked_at + interval '1 hour');
END;
$$;

REVOKE ALL ON FUNCTION craft_queue.renew_verification_pickup(text,text,uuid) FROM PUBLIC;

COMMIT;
