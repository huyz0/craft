BEGIN;

CREATE FUNCTION craft_queue.pending_verifications(actor text, repo text, after_id text, requested_limit integer)
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

COMMIT;
