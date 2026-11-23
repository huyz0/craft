BEGIN;

CREATE FUNCTION craft_queue.runner_setup(runner text, mapped_repositories text[]) RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT jsonb_build_object(
        'runner_id', runner,
        'mode', craft_queue.runner_status(runner)->>'mode',
        'queue_paused', settings.paused,
        'enrolled_repositories', COALESCE((
            SELECT jsonb_agg(e.repository ORDER BY e.repository)
            FROM craft_queue.runner_repositories AS e
            WHERE e.runner_id = runner AND e.repository = ANY(mapped_repositories)
        ), '[]'::jsonb)
    ) FROM craft_queue.control AS settings;
$$;

REVOKE ALL ON FUNCTION craft_queue.runner_setup(text, text[]) FROM PUBLIC;

COMMIT;
