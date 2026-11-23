BEGIN;

CREATE FUNCTION craft_queue.review_feedback_target(reviewer text, handoff_id text) RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT jsonb_build_object(
        'draft', jsonb_build_object('reference', p.result_url, 'head', h.head),
        'base', p.base
    )
    FROM craft_queue.review_handoffs AS h
    JOIN craft_queue.review_inbox_grants AS g ON g.grant_id = h.grant_id
    JOIN craft_queue.work AS w ON w.id = h.work_id AND w.repository = g.repository
    JOIN craft_queue.publications AS p ON p.action_key = h.publication_key
    WHERE h.id = handoff_id AND h.reviewer_id = reviewer AND g.reviewer_id = reviewer
      AND g.revoked_at IS NULL AND g.expires_at > statement_timestamp()
      AND p.action_key LIKE 'github-review:%'
      AND (
          (h.decision IS NULL AND p.action_key = craft_queue.current_review_draft(
              h.work_id, h.generation, h.head, h.verification_sequence
          ))
          OR h.decision = 'changes_requested'
      );
$$;

REVOKE ALL ON FUNCTION craft_queue.review_feedback_target(text, text) FROM PUBLIC;

COMMIT;
