BEGIN;

ALTER FUNCTION craft_queue.review_inbox(text, text, integer, text) RENAME TO review_inbox_before_repair_waits;

CREATE FUNCTION craft_queue.review_inbox(repo text, owner_name text, requested_limit integer, reviewer text) RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    WITH inbox AS MATERIALIZED (
        SELECT craft_queue.review_inbox_before_repair_waits($1, $2, $3, $4) AS result
    )
    SELECT CASE WHEN $4 IS NOT NULL THEN inbox.result ELSE jsonb_set(inbox.result, '{items}', (
        SELECT COALESCE(jsonb_agg(
            CASE WHEN pending.total > 0 THEN jsonb_set(entry.item, '{pending_repair}',
                (entry.item->'pending_repair') || jsonb_build_object(
                    'outstanding_assignments', pending.total,
                    'blocker', COALESCE(entry.item #>> '{pending_repair,blocker}', 'awaiting_reviews')
                ))
            ELSE entry.item END ORDER BY entry.position
        ), '[]'::jsonb)
        FROM jsonb_array_elements(inbox.result->'items') WITH ORDINALITY AS entry(item, position)
        LEFT JOIN LATERAL (
            SELECT count(*) AS total
            FROM craft_queue.review_handoffs AS recorded
            JOIN craft_queue.review_handoffs AS assignment
              ON assignment.publication_key = recorded.publication_key
             AND assignment.verification_sequence = recorded.verification_sequence
            JOIN craft_queue.review_inbox_grants AS scope ON scope.grant_id = assignment.grant_id
            WHERE recorded.id = entry.item #>> '{pending_repair,handoff_id}'
              AND assignment.decision IS NULL AND scope.revoked_at IS NULL
              AND scope.expires_at > statement_timestamp()
        ) AS pending ON entry.item ? 'pending_repair'
    )) END FROM inbox;
$$;

REVOKE ALL ON FUNCTION craft_queue.review_inbox(text, text, integer, text) FROM PUBLIC;

COMMIT;
