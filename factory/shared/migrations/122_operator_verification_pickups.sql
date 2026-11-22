BEGIN;

ALTER FUNCTION craft_queue.operator_brief(text,integer,integer) RENAME TO operator_brief_before_verifier_pickups;

CREATE FUNCTION craft_queue.operator_brief(repo_filter text, usage_hours integer, item_limit integer)
RETURNS jsonb LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    WITH brief AS MATERIALIZED (
        SELECT craft_queue.operator_brief_before_verifier_pickups(repo_filter, usage_hours, item_limit) AS snapshot
    )
    SELECT jsonb_set(snapshot, '{review,items}', COALESCE((
        SELECT jsonb_agg(
            CASE WHEN pickup.id IS NULL THEN item
                ELSE item || jsonb_build_object('verification_pickup', jsonb_build_object(
                    'verifier', pickup.verifier, 'picked_at', pickup.picked_at, 'expires_at', pickup.expires_at))
            END ORDER BY position)
        FROM jsonb_array_elements(snapshot#>'{review,items}') WITH ORDINALITY AS entries(item, position)
        LEFT JOIN LATERAL (
            SELECT p.id, p.verifier, p.picked_at, p.expires_at
            FROM craft_queue.work AS w
            JOIN craft_queue.verification_pickups AS p ON p.work_id = w.id AND p.generation = w.generation
            WHERE item->>'next_action' = 'verify_candidate'
              AND w.id = item->>'work_id' AND w.candidate = item->>'head'
              AND p.released_at IS NULL AND p.expires_at > statement_timestamp()
              AND NOT EXISTS (SELECT 1 FROM craft_queue.recoveries AS recovery WHERE recovery.started_at >= p.picked_at)
            ORDER BY p.picked_at DESC, p.id DESC LIMIT 1
        ) AS pickup ON true
    ), '[]'::jsonb), false)
    FROM brief;
$$;

REVOKE ALL ON FUNCTION craft_queue.operator_brief(text,integer,integer) FROM PUBLIC;

COMMIT;
