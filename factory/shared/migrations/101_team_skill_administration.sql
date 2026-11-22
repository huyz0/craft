BEGIN;

ALTER TABLE craft_queue.team_skill_releases
    ADD COLUMN registered_by text,
    ADD COLUMN registered_at timestamptz,
    ADD COLUMN revoked_by text;

CREATE FUNCTION craft_queue.register_team_skill(actor_id text, proposed jsonb)
RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    incoming craft_queue.team_skill_releases%ROWTYPE;
    saved craft_queue.team_skill_releases%ROWTYPE;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    incoming := jsonb_populate_record(NULL::craft_queue.team_skill_releases, proposed);
    IF incoming.revision IS DISTINCT FROM
        'sha256:' || encode(sha256(convert_to(incoming.markdown, 'UTF8')), 'hex') THEN RETURN NULL; END IF;
    SELECT s.* INTO saved FROM craft_queue.team_skill_releases AS s
    WHERE s.location = incoming.location AND s.revision = incoming.revision;
    IF FOUND THEN
        IF saved.revoked_at IS NULL AND saved.registered_by = actor_id
            AND saved.markdown = incoming.markdown AND saved.owner_name = incoming.owner_name
            AND saved.approval_reference = incoming.approval_reference
            AND saved.repositories = incoming.repositories THEN
            RETURN to_jsonb(saved) - 'markdown';
        END IF;
        RETURN NULL;
    END IF;
    IF EXISTS (SELECT 1 FROM craft_queue.recoveries WHERE finished_at IS NULL)
        OR EXISTS (SELECT 1 FROM craft_queue.team_skill_releases AS s WHERE s.location = incoming.location
            AND (s.owner_name <> incoming.owner_name OR NOT (
                s.repositories @> incoming.repositories AND s.repositories <@ incoming.repositories
            ))) THEN RETURN NULL; END IF;
    INSERT INTO craft_queue.team_skill_releases (
        location, revision, markdown, owner_name, approval_reference, repositories, registered_by, registered_at
    ) VALUES (incoming.location, incoming.revision, incoming.markdown, incoming.owner_name,
        incoming.approval_reference, incoming.repositories, actor_id, clock_timestamp())
    RETURNING * INTO saved;
    RETURN to_jsonb(saved) - 'markdown';
END;
$$;

CREATE FUNCTION craft_queue.inspect_team_skill(skill_location text, skill_revision text) RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT to_jsonb(s) - 'markdown' FROM craft_queue.team_skill_releases AS s
    WHERE s.location = skill_location AND s.revision = skill_revision;
$$;

CREATE FUNCTION craft_queue.withdraw_team_skill(actor_id text, skill_location text, skill_revision text, evidence_ref text)
RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    saved craft_queue.team_skill_releases%ROWTYPE;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    SELECT s.* INTO saved FROM craft_queue.team_skill_releases AS s
    WHERE s.location = skill_location AND s.revision = skill_revision;
    IF NOT FOUND THEN RETURN NULL; END IF;
    IF saved.revoked_at IS NOT NULL THEN
        IF saved.revoked_by = actor_id AND saved.revocation_reference = evidence_ref
            THEN RETURN to_jsonb(saved) - 'markdown'; END IF;
        RETURN NULL;
    END IF;
    IF NOT craft_queue.revoke_team_skill(skill_location, skill_revision, evidence_ref) THEN RETURN NULL; END IF;
    UPDATE craft_queue.team_skill_releases AS s SET revoked_by = actor_id
    WHERE s.location = skill_location AND s.revision = skill_revision RETURNING * INTO saved;
    RETURN to_jsonb(saved) - 'markdown';
END;
$$;

REVOKE ALL ON FUNCTION craft_queue.register_team_skill(text,jsonb) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.inspect_team_skill(text,text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.withdraw_team_skill(text,text,text,text) FROM PUBLIC;

COMMIT;
