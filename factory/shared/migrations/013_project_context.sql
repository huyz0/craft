BEGIN;

CREATE FUNCTION craft_queue.project_context_revision(
    kind text, source_reference text, source_revision text, approval_reference text, excerpt text
) RETURNS text
LANGUAGE sql IMMUTABLE STRICT SET search_path = pg_catalog AS $$
    SELECT 'sha256:' || encode(sha256(convert_to(kind || E'\n' || source_reference || E'\n'
        || source_revision || E'\n' || approval_reference || E'\n' || excerpt, 'UTF8')), 'hex');
$$;

CREATE TABLE craft_queue.project_context_releases (
    repository text NOT NULL CHECK (length(repository) BETWEEN 1 AND 200),
    location text NOT NULL CHECK (location ~ '^project:[a-z0-9][a-z0-9_-]{0,79}$'),
    revision text NOT NULL,
    excerpt text NOT NULL CHECK (octet_length(convert_to(excerpt, 'UTF8')) BETWEEN 1 AND 4096),
    kind text NOT NULL CHECK (kind IN ('decision', 'runbook')),
    source_reference text NOT NULL CHECK (length(btrim(source_reference)) BETWEEN 1 AND 500
        AND source_reference !~ '[[:cntrl:]]'),
    source_revision text NOT NULL CHECK (length(btrim(source_revision)) BETWEEN 1 AND 300
        AND source_revision !~ '[[:cntrl:]]'),
    approval_reference text NOT NULL CHECK (length(btrim(approval_reference)) BETWEEN 1 AND 500
        AND approval_reference !~ '[[:cntrl:]]'),
    retired_at timestamptz,
    retirement_reference text CHECK (length(btrim(retirement_reference)) BETWEEN 1 AND 500),
    PRIMARY KEY (repository, location, revision),
    CHECK (revision = craft_queue.project_context_revision(
        kind, source_reference, source_revision, approval_reference, excerpt)),
    CHECK ((retired_at IS NULL) = (retirement_reference IS NULL))
);

CREATE UNIQUE INDEX active_project_context
    ON craft_queue.project_context_releases(repository, location) WHERE retired_at IS NULL;

CREATE FUNCTION craft_queue.project_context_pins(payload jsonb)
RETURNS TABLE(location text, revision text)
LANGUAGE sql IMMUTABLE SET search_path = pg_catalog AS $$
    SELECT entry->>'location', entry->>'revision'
    FROM jsonb_array_elements(CASE WHEN jsonb_typeof(payload->'context') = 'array'
        THEN payload->'context' ELSE '[]'::jsonb END) AS item(entry)
    WHERE entry->>'location' LIKE 'project:%';
$$;

CREATE FUNCTION craft_queue.project_context_allowed(repo text, payload jsonb) RETURNS boolean
LANGUAGE sql STABLE SET search_path = pg_catalog AS $$
    SELECT count(*) <= 4 AND count(*) = count(DISTINCT pin.location) AND count(*) = count(release.location)
    FROM craft_queue.project_context_pins(payload) AS pin
    LEFT JOIN craft_queue.project_context_releases AS release
      ON release.repository = repo AND release.location = pin.location AND release.revision = pin.revision
      AND release.retired_at IS NULL;
$$;

CREATE OR REPLACE FUNCTION craft_queue.guard_work_skill() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog AS $$
BEGIN
    IF NEW.state IN ('queued', 'leased', 'candidate') THEN
        IF NOT craft_queue.team_skill_allowed(NEW.repository, NEW.payload) THEN
            RAISE EXCEPTION 'team skill release is unavailable to this repository';
        END IF;
        IF NOT craft_queue.project_context_allowed(NEW.repository, NEW.payload) THEN
            RAISE EXCEPTION 'project context pins are unavailable to this repository';
        END IF;
    END IF;
    RETURN NEW;
END;
$$;

CREATE OR REPLACE FUNCTION craft_queue.guard_publication_skill() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog AS $$
BEGIN
    IF NEW.state IN ('reserved', 'sending') AND NOT EXISTS (
        SELECT 1 FROM craft_queue.work AS w WHERE w.id = NEW.work_id
          AND craft_queue.team_skill_allowed(w.repository, w.payload)
          AND craft_queue.project_context_allowed(w.repository, w.payload)
    ) THEN
        RAISE EXCEPTION 'publication requires active skill and context releases';
    END IF;
    RETURN NEW;
END;
$$;

CREATE FUNCTION craft_queue.project_context_for_lease(
    runner text, order_id text, lease_generation bigint
) RETURNS jsonb
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT (
        SELECT COALESCE(jsonb_agg(jsonb_build_object(
            'location', c.location, 'revision', c.revision, 'excerpt', c.excerpt, 'kind', c.kind,
            'source_reference', c.source_reference, 'source_revision', c.source_revision,
            'approval_reference', c.approval_reference
        ) ORDER BY c.location), '[]'::jsonb)
        FROM craft_queue.project_context_pins(w.payload) AS pin
        JOIN craft_queue.project_context_releases AS c
          ON c.repository = w.repository AND c.location = pin.location AND c.revision = pin.revision
         AND c.retired_at IS NULL
    )
    FROM craft_queue.work AS w
    JOIN craft_queue.attempts AS a ON a.work_id = w.id AND a.generation = w.generation
    JOIN craft_queue.runner_repositories AS enrolled
      ON enrolled.repository = w.repository AND enrolled.runner_id = a.runner_id
    WHERE w.id = order_id AND w.generation = lease_generation AND w.state = 'leased'
      AND a.runner_id = runner AND a.outcome = 'leased' AND a.expires_at > statement_timestamp()
      AND craft_queue.project_context_allowed(w.repository, w.payload);
$$;

CREATE FUNCTION craft_queue.retire_project_context(
    repo text, context_location text, context_revision text, evidence_ref text
) RETURNS boolean
LANGUAGE plpgsql SET search_path = pg_catalog AS $$
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    UPDATE craft_queue.project_context_releases AS release
    SET retired_at = clock_timestamp(), retirement_reference = evidence_ref
    WHERE release.repository = repo AND release.location = context_location
      AND release.revision = context_revision AND release.retired_at IS NULL;
    IF NOT FOUND THEN
        RETURN EXISTS (
            SELECT 1 FROM craft_queue.project_context_releases AS release WHERE release.repository = repo
              AND release.location = context_location AND release.revision = context_revision
              AND release.retirement_reference = evidence_ref
        );
    END IF;

    WITH cancelled AS (
        UPDATE craft_queue.work AS w
        SET state = 'cancelled', cancellation_reason = 'Approved project context retired'
        WHERE w.repository = repo AND w.state IN ('queued', 'leased', 'candidate')
          AND EXISTS (
              SELECT 1 FROM craft_queue.project_context_pins(w.payload) AS pin
              WHERE pin.location = context_location AND pin.revision = context_revision
          )
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

CREATE FUNCTION craft_queue.claim(runner text, request uuid, context_capable boolean)
RETURNS TABLE (work_id text, repository text, generation bigint, expires_at timestamptz, work_order jsonb)
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    settings craft_queue.control%ROWTYPE;
    selected craft_queue.work%ROWTYPE;
    previous craft_queue.attempts%ROWTYPE;
    now_at timestamptz;
    review_free boolean;
    tokens_free numeric;
    deadline timestamptz;
BEGIN
    SELECT * INTO STRICT settings FROM craft_queue.control FOR UPDATE;
    now_at := clock_timestamp();
    SELECT * INTO previous FROM craft_queue.attempts AS a
    WHERE a.runner_id = runner AND a.request_id = request;
    IF FOUND THEN
        RETURN QUERY SELECT w.id, w.repository, w.generation, previous.expires_at,
            (w.payload - 'repository') || CASE WHEN w.repair_from_generation IS NULL
                THEN '{}'::jsonb ELSE '{"shared_repair":true}'::jsonb END
        FROM craft_queue.work AS w
        JOIN craft_queue.runner_repositories AS grant_row ON grant_row.repository = w.repository
        WHERE w.id = previous.work_id AND w.generation = previous.generation
          AND w.state = 'leased' AND previous.outcome = 'leased' AND previous.expires_at > now_at
          AND grant_row.runner_id = runner
          AND (context_capable OR NOT EXISTS (SELECT 1 FROM craft_queue.project_context_pins(w.payload)));
        RETURN;
    END IF;
    IF settings.paused THEN RETURN; END IF;

    SELECT COUNT(*) < settings.review_limit INTO review_free
    FROM craft_queue.work AS w WHERE w.state IN ('leased', 'candidate')
      OR (w.state = 'queued' AND w.repair_from_generation IS NOT NULL);
    SELECT settings.token_budget - COALESCE(SUM(COALESCE(a.reported_tokens, a.token_limit)), 0)
    INTO tokens_free FROM craft_queue.attempts AS a
    WHERE a.started_at > now_at - interval '24 hours'
       OR (a.outcome = 'leased' AND a.expires_at > now_at);

    SELECT w.* INTO selected
    FROM craft_queue.work AS w
    JOIN craft_queue.runner_repositories AS grant_row ON grant_row.repository = w.repository
    LEFT JOIN craft_queue.attempts AS a ON a.work_id = w.id AND a.generation = w.generation
    WHERE grant_row.runner_id = runner AND w.reserved_tokens <= tokens_free
      AND (context_capable OR NOT EXISTS (SELECT 1 FROM craft_queue.project_context_pins(w.payload)))
      AND ((w.state = 'queued' AND (review_free OR w.repair_from_generation IS NOT NULL))
        OR (w.state = 'leased' AND a.outcome = 'leased' AND a.expires_at <= now_at))
    ORDER BY (w.state = 'leased' OR w.repair_from_generation IS NOT NULL) DESC, w.created_at, w.id
    LIMIT 1;
    IF NOT FOUND THEN RETURN; END IF;

    UPDATE craft_queue.attempts AS a SET outcome = 'expired'
    WHERE a.work_id = selected.id AND a.generation = selected.generation AND a.outcome = 'leased';
    UPDATE craft_queue.work AS w SET state = 'leased', generation = w.generation + 1
    WHERE w.id = selected.id RETURNING w.* INTO selected;
    deadline := now_at + make_interval(secs => settings.lease_seconds);
    INSERT INTO craft_queue.attempts (work_id, generation, runner_id, request_id, expires_at, started_at, token_limit)
    VALUES (selected.id, selected.generation, runner, request, deadline, now_at, selected.reserved_tokens);
    RETURN QUERY SELECT selected.id, selected.repository, selected.generation, deadline,
        (selected.payload - 'repository') || CASE WHEN selected.repair_from_generation IS NULL
            THEN '{}'::jsonb ELSE '{"shared_repair":true}'::jsonb END;
END;
$$;

CREATE OR REPLACE FUNCTION craft_queue.claim(runner text, request uuid)
RETURNS TABLE (work_id text, repository text, generation bigint, expires_at timestamptz, work_order jsonb)
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT * FROM craft_queue.claim(runner, request, false);
$$;

REVOKE ALL ON FUNCTION craft_queue.claim(text, uuid, boolean) FROM PUBLIC;

REVOKE ALL ON FUNCTION craft_queue.project_context_revision(text, text, text, text, text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.project_context_pins(jsonb) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.project_context_allowed(text, jsonb) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.project_context_for_lease(text, text, bigint) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.retire_project_context(text, text, text, text) FROM PUBLIC;
COMMIT;
