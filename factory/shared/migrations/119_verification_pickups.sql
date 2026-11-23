BEGIN;

CREATE TABLE craft_queue.verification_pickups (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    work_id text NOT NULL REFERENCES craft_queue.work(id) ON DELETE CASCADE,
    generation bigint NOT NULL,
    verifier text NOT NULL,
    picked_at timestamptz NOT NULL,
    expires_at timestamptz NOT NULL,
    released_at timestamptz
);
CREATE INDEX verification_pickups_active ON craft_queue.verification_pickups (work_id, generation, expires_at)
WHERE released_at IS NULL;

CREATE OR REPLACE FUNCTION craft_queue.pending_verifications(actor text, repo text, after_id text, requested_limit integer)
RETURNS jsonb LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    WITH targets AS MATERIALIZED (
        SELECT w.id AS work_id, w.generation, w.candidate AS head, p.digest AS pack_digest,
               left(w.payload->>'issue', 100) AS issue,
               (w.payload->'require_native_checks' IS NOT DISTINCT FROM 'true'::jsonb) AS requires_native_checks
        FROM craft_queue.work AS w
        JOIN craft_queue.attempts AS a ON a.work_id = w.id AND a.generation = w.generation
        JOIN craft_queue.candidate_packs AS p ON p.work_id = w.id AND p.generation = w.generation
            AND p.digest = w.candidate_evidence
        WHERE requested_limit BETWEEN 1 AND 50 AND w.repository = repo AND w.state = 'candidate'
          AND (after_id IS NULL OR w.id COLLATE "C" > after_id COLLATE "C")
          AND a.runner_id <> actor AND a.outcome = 'candidate'
          AND NOT EXISTS (
              SELECT 1 FROM craft_queue.verification_pickups AS pickup
              WHERE pickup.work_id = w.id AND pickup.generation = w.generation
                AND pickup.released_at IS NULL AND pickup.expires_at > statement_timestamp()
                AND NOT EXISTS (SELECT 1 FROM craft_queue.recoveries AS recovery
                    WHERE recovery.started_at >= pickup.picked_at)
          )
          AND craft_queue.standing_delegation_current(w.id, statement_timestamp())
          AND NOT EXISTS (SELECT 1 FROM craft_queue.verifications AS v
              WHERE v.work_id = w.id AND v.generation = w.generation
                AND v.head = w.candidate AND v.digest = p.digest)
          AND NOT EXISTS (SELECT 1 FROM craft_queue.publications AS publication
              WHERE publication.work_id = w.id AND publication.state IN ('sending', 'uncertain'))
        ORDER BY w.id COLLATE "C"
        LIMIT requested_limit + 1
    ), page AS MATERIALIZED (
        SELECT * FROM targets ORDER BY work_id COLLATE "C" LIMIT requested_limit
    )
    SELECT jsonb_build_object(
        'repository', repo,
        'items', COALESCE((SELECT jsonb_agg(to_jsonb(page) ORDER BY work_id COLLATE "C") FROM page), '[]'::jsonb),
        'next_after_work_id', CASE WHEN (SELECT count(*) FROM targets) > requested_limit
            THEN (SELECT max(work_id COLLATE "C") FROM page) END
    );
$$;

REVOKE ALL ON FUNCTION craft_queue.pending_verifications(text,text,text,integer) FROM PUBLIC;

CREATE FUNCTION craft_queue.claim_pending_verification(actor text, repo text, after_id text)
RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    target jsonb;
    claimed craft_queue.verification_pickups%ROWTYPE;
    now_at timestamptz;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    IF EXISTS (SELECT 1 FROM craft_queue.recoveries WHERE finished_at IS NULL) THEN RETURN NULL; END IF;
    target := craft_queue.pending_verifications(actor, repo, after_id, 1)#>'{items,0}';
    IF target IS NULL THEN RETURN NULL; END IF;
    now_at := clock_timestamp();
    INSERT INTO craft_queue.verification_pickups(work_id, generation, verifier, picked_at, expires_at)
    VALUES (target->>'work_id', (target->>'generation')::bigint, actor, now_at, now_at + interval '10 minutes')
    RETURNING * INTO claimed;
    RETURN target || jsonb_build_object('repository', repo,
        'pickup_id', claimed.id, 'pickup_expires_at', claimed.expires_at);
END;
$$;

CREATE FUNCTION craft_queue.release_verification_pickup(actor text, repo text, pickup_id uuid)
RETURNS boolean LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    WITH released AS (
        UPDATE craft_queue.verification_pickups AS pickup
        SET released_at = COALESCE(pickup.released_at, clock_timestamp())
        FROM craft_queue.work AS w
        WHERE pickup.id = pickup_id AND pickup.verifier = actor AND w.id = pickup.work_id AND w.repository = repo
        RETURNING pickup.id
    )
    SELECT EXISTS (SELECT 1 FROM released);
$$;

REVOKE ALL ON FUNCTION craft_queue.claim_pending_verification(text,text,text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.release_verification_pickup(text,text,uuid) FROM PUBLIC;

COMMIT;
