BEGIN;

CREATE FUNCTION craft_queue.review_wait_summary() RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT jsonb_build_object(
        'pending_review_assignments', count(*),
        'oldest_pending_review_seconds', floor(extract(epoch FROM statement_timestamp() - min(h.requested_at)))
    )
    FROM craft_queue.review_handoffs AS h
    JOIN craft_queue.review_inbox_grants AS g ON g.grant_id = h.grant_id
    WHERE h.decision IS NULL
      AND g.revoked_at IS NULL AND g.expires_at > statement_timestamp()
      AND h.publication_key = craft_queue.current_review_draft(
          h.work_id, h.generation, h.head, h.verification_sequence
      );
$$;

REVOKE ALL ON FUNCTION craft_queue.review_wait_summary() FROM PUBLIC;

COMMIT;
