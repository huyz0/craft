BEGIN;

ALTER TABLE craft_queue.project_context_releases
    ADD COLUMN registered_by text,
    ADD COLUMN registered_at timestamptz,
    ADD COLUMN replaces_revision text;

CREATE FUNCTION craft_queue.register_project_context(actor text, desired jsonb, expected_revision text)
RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    incoming craft_queue.project_context_releases%ROWTYPE;
    previous craft_queue.project_context_releases%ROWTYPE;
    saved craft_queue.project_context_releases%ROWTYPE;
BEGIN
    incoming := jsonb_populate_record(NULL::craft_queue.project_context_releases, desired);
    IF incoming.revision IS NULL OR incoming.revision IS DISTINCT FROM craft_queue.project_context_revision(
        incoming.kind, incoming.source_reference, incoming.source_revision,
        incoming.approval_reference, incoming.excerpt
    ) THEN RETURN NULL; END IF;

    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    SELECT * INTO saved FROM craft_queue.project_context_releases AS release
    WHERE release.repository = incoming.repository AND release.location = incoming.location
      AND release.revision = incoming.revision;
    IF FOUND THEN
        IF saved.retired_at IS NULL AND saved.registered_by = actor
           AND saved.replaces_revision IS NOT DISTINCT FROM expected_revision THEN
            RETURN to_jsonb(saved) - 'excerpt';
        END IF;
        RETURN NULL;
    END IF;
    IF EXISTS (SELECT 1 FROM craft_queue.recoveries WHERE finished_at IS NULL) THEN RETURN NULL; END IF;

    SELECT * INTO previous FROM craft_queue.project_context_releases AS release
    WHERE release.repository = incoming.repository AND release.location = incoming.location
      AND release.retired_at IS NULL FOR UPDATE;
    IF previous.revision IS DISTINCT FROM expected_revision THEN RETURN NULL; END IF;
    IF expected_revision IS NOT NULL AND NOT craft_queue.retire_project_context(
        incoming.repository, incoming.location, expected_revision, incoming.approval_reference
    ) THEN RETURN NULL; END IF;

    INSERT INTO craft_queue.project_context_releases (
        repository, location, revision, excerpt, kind, source_reference, source_revision,
        approval_reference, registered_by, registered_at, replaces_revision
    ) VALUES (
        incoming.repository, incoming.location, incoming.revision, incoming.excerpt, incoming.kind,
        incoming.source_reference, incoming.source_revision, incoming.approval_reference,
        actor, clock_timestamp(), expected_revision
    ) RETURNING * INTO saved;
    RETURN to_jsonb(saved) - 'excerpt';
END;
$$;

REVOKE ALL ON FUNCTION craft_queue.register_project_context(text, jsonb, text) FROM PUBLIC;

COMMIT;
