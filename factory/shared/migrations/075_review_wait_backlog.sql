BEGIN;

CREATE VIEW craft_queue.current_review_waits AS
    SELECT h.id, h.work_id, h.reviewer_id, h.head, h.requested_at, h.publication_key
    FROM craft_queue.review_handoffs AS h
    JOIN craft_queue.review_inbox_grants AS g ON g.grant_id = h.grant_id
    WHERE h.decision IS NULL
      AND g.revoked_at IS NULL AND g.expires_at > statement_timestamp()
      AND h.publication_key = craft_queue.current_review_draft(
          h.work_id, h.generation, h.head, h.verification_sequence
      );

REVOKE ALL ON craft_queue.current_review_waits FROM PUBLIC;

CREATE OR REPLACE FUNCTION craft_queue.review_wait_summary() RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT jsonb_build_object(
        'pending_review_assignments', count(*),
        'oldest_pending_review_seconds', floor(extract(epoch FROM statement_timestamp() - min(requested_at)))
    )
    FROM craft_queue.current_review_waits;
$$;

CREATE FUNCTION craft_queue.review_wait_backlog(repo text, maximum integer) RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    WITH waiting AS MATERIALIZED (
        SELECT h.id AS handoff_id, h.work_id, h.reviewer_id, w.repository, h.head, h.requested_at,
               floor(extract(epoch FROM statement_timestamp() - h.requested_at))::bigint AS wait_seconds,
               p.result_url AS draft_url
        FROM craft_queue.current_review_waits AS h
        JOIN craft_queue.work AS w ON w.id = h.work_id
        JOIN craft_queue.publications AS p ON p.action_key = h.publication_key
        WHERE repo IS NULL OR w.repository = repo
    ), oldest AS (
        SELECT * FROM waiting
        ORDER BY requested_at, handoff_id LIMIT least(greatest(maximum, 1), 50)
    )
    SELECT jsonb_build_object(
        'total', (SELECT count(*) FROM waiting),
        'oldest_seconds', (SELECT max(wait_seconds) FROM waiting),
        'assignments', COALESCE((SELECT jsonb_agg(to_jsonb(oldest) ORDER BY requested_at, handoff_id)
                                 FROM oldest), '[]'::jsonb)
    );
$$;

REVOKE ALL ON FUNCTION craft_queue.review_wait_backlog(text, integer) FROM PUBLIC;

COMMIT;
