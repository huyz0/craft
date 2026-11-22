BEGIN;

CREATE INDEX release_observer_inbox_members ON craft_queue.series_members (observer_id, work_id);

CREATE FUNCTION craft_queue.release_observer_inbox(
    actor_id text, repo text, after_id text, page_size integer, include_closed boolean
) RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    WITH matches AS MATERIALIZED (
        SELECT member.work_id
        FROM craft_queue.series_members AS member
        JOIN craft_queue.work AS w ON w.id = member.work_id
        WHERE member.observer_id = actor_id
            AND (repo IS NULL OR w.repository = repo)
            AND (after_id IS NULL OR member.work_id > after_id)
            AND (include_closed OR (w.state <> 'cancelled'
                AND (w.state <> 'released' OR NOT craft_queue.series_release_current(w.id))))
        ORDER BY member.work_id LIMIT (page_size + 1)
    ), page AS (
        SELECT work_id FROM matches ORDER BY work_id LIMIT page_size
    )
    SELECT jsonb_build_object(
        'items', COALESCE((
            SELECT jsonb_agg(craft_queue.release_observer_status(actor_id, work_id) ORDER BY work_id)
            FROM page
        ), '[]'::jsonb),
        'next_after', CASE WHEN (SELECT count(*) FROM matches) > page_size THEN
            (SELECT max(work_id) FROM page) END
    );
$$;

REVOKE ALL ON FUNCTION craft_queue.release_observer_inbox(text,text,text,integer,boolean) FROM PUBLIC;

COMMIT;
