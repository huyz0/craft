BEGIN;

CREATE OR REPLACE FUNCTION craft_queue.runner_setup(runner text, mapped_repositories text[]) RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    WITH enrolled AS (
        SELECT enrollment.repository
        FROM craft_queue.runner_repositories AS enrollment
        WHERE enrollment.runner_id = runner AND enrollment.repository = ANY(mapped_repositories)
    )
    SELECT jsonb_build_object(
        'runner_id', runner,
        'mode', craft_queue.runner_status(runner)->>'mode',
        'queue_paused', settings.paused,
        'enrolled_repositories', COALESCE((
            SELECT jsonb_agg(repository ORDER BY repository) FROM enrolled
        ), '[]'::jsonb),
        'claude_approved_repositories', COALESCE((
            SELECT jsonb_agg(enrolled.repository ORDER BY enrolled.repository)
            FROM enrolled
            WHERE EXISTS (
                SELECT 1 FROM craft_queue.claude_repository_approvals AS approval
                WHERE approval.repository = enrolled.repository
                  AND approval.revoked_at IS NULL AND approval.expires_at > statement_timestamp()
            )
        ), '[]'::jsonb)
    ) FROM craft_queue.control AS settings;
$$;

COMMIT;
