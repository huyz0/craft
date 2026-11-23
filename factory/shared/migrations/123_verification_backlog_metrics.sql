BEGIN;

ALTER TABLE craft_queue.candidate_packs ADD COLUMN submitted_at timestamptz;
ALTER TABLE craft_queue.candidate_packs ALTER COLUMN submitted_at SET DEFAULT clock_timestamp();

CREATE VIEW craft_queue.first_verification_candidates AS
    SELECT w.id AS work_id, w.repository, w.generation, w.candidate AS head, p.digest AS pack_digest,
        left(w.payload->>'issue', 100) AS issue,
        (w.payload->'require_native_checks' IS NOT DISTINCT FROM 'true'::jsonb) AS requires_native_checks,
        a.runner_id, p.submitted_at,
        EXISTS (
            SELECT 1 FROM craft_queue.verification_pickups AS pickup
            WHERE pickup.work_id = w.id AND pickup.generation = w.generation
              AND pickup.released_at IS NULL AND pickup.expires_at > statement_timestamp()
              AND NOT EXISTS (SELECT 1 FROM craft_queue.recoveries AS recovery
                  WHERE recovery.started_at >= pickup.picked_at)
        ) AS picked
    FROM craft_queue.work AS w
    JOIN craft_queue.attempts AS a ON a.work_id = w.id AND a.generation = w.generation
    JOIN craft_queue.candidate_packs AS p ON p.work_id = w.id AND p.generation = w.generation
        AND p.digest = w.candidate_evidence
    WHERE w.state = 'candidate' AND a.outcome = 'candidate'
      AND craft_queue.standing_delegation_current(w.id, statement_timestamp())
      AND NOT EXISTS (SELECT 1 FROM craft_queue.verifications AS v
          WHERE v.work_id = w.id AND v.generation = w.generation AND v.head = w.candidate AND v.digest = p.digest)
      AND NOT EXISTS (SELECT 1 FROM craft_queue.publications AS publication
          WHERE publication.work_id = w.id AND publication.state IN ('sending', 'uncertain'));

CREATE OR REPLACE FUNCTION craft_queue.pending_verifications(actor text, repo text, after_id text, requested_limit integer)
RETURNS jsonb LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    WITH targets AS MATERIALIZED (
        SELECT work_id, generation, head, pack_digest, issue, requires_native_checks
        FROM craft_queue.first_verification_candidates
        WHERE requested_limit BETWEEN 1 AND 50 AND repository = repo AND runner_id <> actor AND NOT picked
          AND (after_id IS NULL OR work_id COLLATE "C" > after_id COLLATE "C")
        ORDER BY work_id COLLATE "C"
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

CREATE FUNCTION craft_queue.verification_wait_summary() RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT jsonb_build_object(
        'waiting_verification_candidates', count(*) FILTER (WHERE NOT picked),
        'active_verification_pickups', count(*) FILTER (WHERE picked),
        'oldest_waiting_verification_seconds',
            floor(extract(epoch FROM statement_timestamp() - min(submitted_at) FILTER (WHERE NOT picked)))
    )
    FROM craft_queue.first_verification_candidates;
$$;

REVOKE ALL ON craft_queue.first_verification_candidates FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.verification_wait_summary() FROM PUBLIC;

COMMIT;
