BEGIN;

CREATE TABLE craft_queue.team_skill_releases (
    location text NOT NULL CHECK (location ~ '^team:[a-z0-9][a-z0-9_-]{0,79}$'),
    revision text NOT NULL,
    markdown text NOT NULL CHECK (octet_length(convert_to(markdown, 'UTF8')) BETWEEN 1 AND 32768),
    owner_name text NOT NULL CHECK (length(btrim(owner_name)) BETWEEN 1 AND 200),
    approval_reference text NOT NULL CHECK (length(btrim(approval_reference)) BETWEEN 1 AND 500),
    repositories text[] NOT NULL CHECK (cardinality(repositories) BETWEEN 1 AND 100),
    revoked_at timestamptz,
    revocation_reference text CHECK (length(btrim(revocation_reference)) BETWEEN 1 AND 500),
    PRIMARY KEY (location, revision),
    CHECK (revision = 'sha256:' || encode(sha256(convert_to(markdown, 'UTF8')), 'hex')),
    CHECK ((revoked_at IS NULL) = (revocation_reference IS NULL))
);

CREATE FUNCTION craft_queue.team_skill_allowed(repo text, payload jsonb) RETURNS boolean
LANGUAGE sql STABLE SET search_path = pg_catalog AS $$
    SELECT COALESCE(payload #>> '{skill,location}', '') NOT LIKE 'team:%'
        OR EXISTS (
            SELECT 1 FROM craft_queue.team_skill_releases AS s
            WHERE s.location = payload #>> '{skill,location}'
              AND s.revision = payload #>> '{skill,revision}'
              AND s.revoked_at IS NULL AND repo = ANY(s.repositories)
        );
$$;

CREATE FUNCTION craft_queue.guard_work_skill() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog AS $$
BEGIN
    IF NEW.state IN ('queued', 'leased', 'candidate')
       AND NOT craft_queue.team_skill_allowed(NEW.repository, NEW.payload) THEN
        RAISE EXCEPTION 'team skill release is unavailable to this repository';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER guard_work_skill BEFORE INSERT OR UPDATE OF state ON craft_queue.work
    FOR EACH ROW EXECUTE FUNCTION craft_queue.guard_work_skill();

CREATE FUNCTION craft_queue.guard_publication_skill() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog AS $$
BEGIN
    IF NEW.state IN ('reserved', 'sending') AND NOT EXISTS (
        SELECT 1 FROM craft_queue.work AS w WHERE w.id = NEW.work_id
          AND craft_queue.team_skill_allowed(w.repository, w.payload)
    ) THEN
        RAISE EXCEPTION 'publication requires an active skill release';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER guard_publication_skill BEFORE INSERT OR UPDATE OF state ON craft_queue.publications
    FOR EACH ROW EXECUTE FUNCTION craft_queue.guard_publication_skill();

CREATE FUNCTION craft_queue.team_skill_for_lease(
    runner text, order_id text, lease_generation bigint
) RETURNS jsonb
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT jsonb_build_object(
        'location', s.location, 'revision', s.revision, 'markdown', s.markdown,
        'owner', s.owner_name, 'approval_reference', s.approval_reference
    )
    FROM craft_queue.work AS w
    JOIN craft_queue.attempts AS a ON a.work_id = w.id AND a.generation = w.generation
    JOIN craft_queue.runner_repositories AS enrollment
      ON enrollment.repository = w.repository AND enrollment.runner_id = a.runner_id
    JOIN craft_queue.team_skill_releases AS s
      ON s.location = w.payload #>> '{skill,location}' AND s.revision = w.payload #>> '{skill,revision}'
    WHERE w.id = order_id AND w.generation = lease_generation AND w.state = 'leased'
      AND a.runner_id = runner AND a.outcome = 'leased' AND a.expires_at > statement_timestamp()
      AND s.revoked_at IS NULL AND w.repository = ANY(s.repositories);
$$;

-- The migration owner runs this operation. The API login has no direct table privileges.
CREATE FUNCTION craft_queue.revoke_team_skill(skill_location text, skill_revision text, evidence_ref text)
RETURNS boolean
LANGUAGE plpgsql SET search_path = pg_catalog AS $$
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    UPDATE craft_queue.team_skill_releases AS s
    SET revoked_at = clock_timestamp(), revocation_reference = evidence_ref
    WHERE s.location = skill_location AND s.revision = skill_revision AND s.revoked_at IS NULL;
    IF NOT FOUND THEN
        RETURN EXISTS (
            SELECT 1 FROM craft_queue.team_skill_releases AS s WHERE s.location = skill_location
              AND s.revision = skill_revision AND s.revocation_reference = evidence_ref
        );
    END IF;
    WITH cancelled AS (
        UPDATE craft_queue.work AS w
        SET state = 'cancelled', cancellation_reason = 'Approved team skill release revoked'
        WHERE w.payload #>> '{skill,location}' = skill_location
          AND w.payload #>> '{skill,revision}' = skill_revision
          AND w.state IN ('queued', 'leased', 'candidate')
          AND NOT EXISTS (
              SELECT 1 FROM craft_queue.publications AS p WHERE p.work_id = w.id
                AND p.state IN ('reserved', 'sending', 'uncertain')
          )
        RETURNING w.id
    )
    UPDATE craft_queue.attempts AS a SET outcome = 'cancelled'
    FROM cancelled WHERE a.work_id = cancelled.id AND a.outcome = 'leased';
    RETURN true;
END;
$$;

REVOKE ALL ON FUNCTION craft_queue.team_skill_allowed(text, jsonb) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.guard_work_skill() FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.guard_publication_skill() FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.team_skill_for_lease(text, text, bigint) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.revoke_team_skill(text, text, text) FROM PUBLIC;
COMMIT;
