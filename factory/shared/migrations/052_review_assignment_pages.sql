BEGIN;

CREATE FUNCTION craft_queue.review_assignment_page(
    reviewer text, repo text, owner_name text, maximum integer, after_id text
)
RETURNS jsonb LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    WITH anchor AS MATERIALIZED (
        SELECT h.requested_at, h.id
        FROM craft_queue.review_handoffs AS h
        JOIN craft_queue.review_inbox_grants AS g ON g.grant_id = h.grant_id
        JOIN craft_queue.work AS w ON w.id = h.work_id
        WHERE h.id = after_id AND h.reviewer_id = reviewer
          AND g.revoked_at IS NULL AND g.expires_at > statement_timestamp()
          AND (repo IS NULL OR w.repository = repo) AND (owner_name IS NULL OR w.payload->>'owner' = owner_name)
    ), page_limit AS (SELECT least(greatest(maximum, 1), 50) AS size), pending AS MATERIALIZED (
        SELECT h.id, h.work_id, w.repository, h.generation, h.head, h.request_evidence,
               h.requested_at, h.queue_repair, p.result_url AS draft_url,
               left(w.payload->>'issue', 100) AS issue, left(w.payload->>'intent', 240) AS intent_excerpt,
               COALESCE(acceptance.excerpts, '[]'::jsonb) AS acceptance_excerpts,
               h.verification_sequence, left(v.evidence, 500) AS verification_evidence
        FROM craft_queue.review_handoffs AS h
        JOIN craft_queue.review_inbox_grants AS g ON g.grant_id = h.grant_id
        JOIN craft_queue.work AS w ON w.id = h.work_id
        JOIN craft_queue.publications AS p ON p.action_key = h.publication_key
        JOIN craft_queue.verifications AS v ON v.sequence = h.verification_sequence
        CROSS JOIN LATERAL (
            SELECT jsonb_agg(left(item.value #>> '{}', 160) ORDER BY item.position) AS excerpts
            FROM jsonb_array_elements(CASE WHEN jsonb_typeof(w.payload->'acceptance') = 'array'
                THEN w.payload->'acceptance' ELSE '[]'::jsonb END) WITH ORDINALITY AS item(value, position)
            WHERE item.position <= 3 AND jsonb_typeof(item.value) = 'string'
        ) AS acceptance
        WHERE h.reviewer_id = reviewer AND h.decision IS NULL
          AND g.revoked_at IS NULL AND g.expires_at > statement_timestamp()
          AND (repo IS NULL OR w.repository = repo) AND (owner_name IS NULL OR w.payload->>'owner' = owner_name)
          AND h.publication_key = craft_queue.current_review_draft(
              h.work_id, h.generation, h.head, h.verification_sequence
          )
          AND (after_id IS NULL OR EXISTS (
              SELECT 1 FROM anchor AS a WHERE (h.requested_at, h.id) > (a.requested_at, a.id)
          ))
        ORDER BY h.requested_at, h.id LIMIT (SELECT size + 1 FROM page_limit)
    ), page AS MATERIALIZED (
        SELECT * FROM pending ORDER BY requested_at, id LIMIT (SELECT size FROM page_limit)
    )
    SELECT jsonb_build_object('assignments', COALESCE((
        SELECT jsonb_agg(to_jsonb(page) ORDER BY requested_at, id) FROM page
    ), '[]'::jsonb)) || CASE WHEN (SELECT count(*) FROM pending) > page_limit.size THEN
        jsonb_build_object('next_after', (SELECT id FROM page ORDER BY requested_at DESC, id DESC LIMIT 1))
        ELSE '{}'::jsonb END
    FROM page_limit WHERE after_id IS NULL OR EXISTS (SELECT 1 FROM anchor);
$$;

CREATE OR REPLACE FUNCTION craft_queue.pending_review_handoffs(reviewer text, repo text, owner_name text, maximum integer)
RETURNS jsonb LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT craft_queue.review_assignment_page(reviewer, repo, owner_name, maximum, NULL::text)->'assignments';
$$;

REVOKE ALL ON FUNCTION craft_queue.review_assignment_page(text, text, text, integer, text) FROM PUBLIC;

COMMIT;
