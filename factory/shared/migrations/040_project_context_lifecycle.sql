BEGIN;

ALTER TABLE craft_queue.project_context_releases ADD COLUMN retired_by text;

CREATE FUNCTION craft_queue.inspect_project_context(repo text, context_location text, context_revision text)
RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    WITH matches AS NOT MATERIALIZED (
        SELECT c.location, to_jsonb(c) - 'excerpt' AS details
        FROM craft_queue.project_context_releases AS c
        WHERE c.repository = repo AND (context_location IS NULL OR c.location = context_location)
          AND CASE WHEN context_revision IS NULL THEN c.retired_at IS NULL
              ELSE c.location = context_location AND c.revision = context_revision END
    )
    SELECT jsonb_build_object(
        'total', (SELECT count(*) FROM matches),
        'items', COALESCE((SELECT jsonb_agg(item.details ORDER BY item.location)
            FROM (SELECT * FROM matches ORDER BY location LIMIT 10) AS item), '[]'::jsonb)
    );
$$;

CREATE FUNCTION craft_queue.withdraw_project_context(
    actor text, repo text, context_location text, context_revision text, evidence_ref text
) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    existing craft_queue.project_context_releases%ROWTYPE;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    SELECT * INTO existing FROM craft_queue.project_context_releases AS c
    WHERE c.repository = repo AND c.location = context_location AND c.revision = context_revision
    FOR UPDATE;
    IF NOT FOUND THEN RETURN NULL; END IF;
    IF existing.retired_at IS NOT NULL THEN
        IF existing.retired_by = actor AND existing.retirement_reference = evidence_ref THEN
            RETURN to_jsonb(existing) - 'excerpt';
        END IF;
        RETURN NULL;
    END IF;
    IF NOT craft_queue.retire_project_context(repo, context_location, context_revision, evidence_ref) THEN
        RETURN NULL;
    END IF;
    UPDATE craft_queue.project_context_releases AS c SET retired_by = actor
    WHERE c.repository = repo AND c.location = context_location AND c.revision = context_revision
    RETURNING * INTO existing;
    RETURN to_jsonb(existing) - 'excerpt';
END;
$$;

REVOKE ALL ON FUNCTION craft_queue.inspect_project_context(text, text, text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.withdraw_project_context(text, text, text, text, text) FROM PUBLIC;

COMMIT;
