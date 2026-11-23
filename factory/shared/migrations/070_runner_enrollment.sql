BEGIN;

CREATE TABLE craft_queue.runner_enrollment_decisions (
    runner_id text NOT NULL,
    repository text NOT NULL,
    revision bigint NOT NULL CHECK (revision > 0),
    enrolled boolean NOT NULL,
    actor text NOT NULL,
    evidence text NOT NULL,
    recorded_at timestamptz NOT NULL,
    fenced_leases bigint NOT NULL CHECK (fenced_leases >= 0),
    PRIMARY KEY (runner_id, repository, revision)
);

CREATE FUNCTION craft_queue.runner_enrollment(runner text, repo text) RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT jsonb_build_object(
        'runner_id', runner, 'repository', repo,
        'enrolled', EXISTS (SELECT 1 FROM craft_queue.runner_repositories AS e
                            WHERE e.runner_id = runner AND e.repository = repo),
        'revision', COALESCE(latest.revision, 0), 'decision', to_jsonb(latest)
    )
    FROM (SELECT 1) AS entry
    LEFT JOIN LATERAL (
        SELECT d.* FROM craft_queue.runner_enrollment_decisions AS d
        WHERE d.runner_id = runner AND d.repository = repo
        ORDER BY d.revision DESC LIMIT 1
    ) AS latest ON true;
$$;

CREATE FUNCTION craft_queue.set_runner_enrollment(
    actor_id text, runner text, repo text, expected_revision bigint, enroll boolean, evidence_ref text
) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    decision craft_queue.runner_enrollment_decisions%ROWTYPE;
    current_revision bigint;
    now_at timestamptz;
    fenced bigint := 0;
BEGIN
    IF COALESCE(runner, '') !~ '^[A-Za-z0-9_-]{1,100}$'
        OR COALESCE(repo, '') !~ '^[A-Za-z0-9][A-Za-z0-9._-]{0,99}/[A-Za-z0-9][A-Za-z0-9._-]{0,99}$'
        OR expected_revision IS NULL OR expected_revision NOT BETWEEN 0 AND 9223372036854775806
        OR enroll IS NULL OR length(btrim(COALESCE(evidence_ref, ''))) NOT BETWEEN 1 AND 500 THEN
        RETURN NULL;
    END IF;
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    SELECT * INTO decision FROM craft_queue.runner_enrollment_decisions AS d
    WHERE d.runner_id = runner AND d.repository = repo AND d.revision = expected_revision + 1;
    IF FOUND THEN
        IF decision.actor = actor_id AND decision.enrolled = enroll AND decision.evidence = evidence_ref THEN
            RETURN to_jsonb(decision);
        END IF;
        RETURN NULL;
    END IF;
    SELECT COALESCE(max(d.revision), 0) INTO current_revision FROM craft_queue.runner_enrollment_decisions AS d
    WHERE d.runner_id = runner AND d.repository = repo;
    IF current_revision <> expected_revision
        OR enroll = EXISTS (SELECT 1 FROM craft_queue.runner_repositories AS e
                            WHERE e.runner_id = runner AND e.repository = repo)
        OR (enroll AND EXISTS (SELECT 1 FROM craft_queue.recoveries AS r WHERE r.finished_at IS NULL)) THEN
        RETURN NULL;
    END IF;

    now_at := clock_timestamp();
    IF enroll THEN
        INSERT INTO craft_queue.runner_repositories (runner_id, repository) VALUES (runner, repo);
    ELSE
        DELETE FROM craft_queue.runner_repositories AS e WHERE e.runner_id = runner AND e.repository = repo;
        WITH stopped AS (
            UPDATE craft_queue.attempts AS a SET expires_at = now_at
            FROM craft_queue.work AS w
            WHERE a.runner_id = runner AND a.work_id = w.id AND a.generation = w.generation
              AND w.repository = repo AND w.state = 'leased' AND a.outcome = 'leased' AND a.expires_at > now_at
            RETURNING a.work_id
        ), withdrawn AS (
            UPDATE craft_queue.profile_fallbacks AS f SET revoked_at = now_at, revoked_by = 'runner-enrollment:' || actor_id
            FROM stopped WHERE f.source_work_id = stopped.work_id AND f.applied_at IS NULL AND f.revoked_at IS NULL
        )
        SELECT count(*) INTO fenced FROM stopped;
        UPDATE craft_queue.runner_claim_polls AS p
        SET available_repositories = array_remove(p.available_repositories, repo)
        WHERE p.runner_id = runner AND repo = ANY(p.available_repositories);
    END IF;

    INSERT INTO craft_queue.runner_enrollment_decisions
        (runner_id, repository, revision, enrolled, actor, evidence, recorded_at, fenced_leases)
    VALUES (runner, repo, current_revision + 1, enroll, actor_id, evidence_ref, now_at, fenced)
    RETURNING * INTO decision;
    RETURN to_jsonb(decision);
END;
$$;

REVOKE ALL ON FUNCTION craft_queue.runner_enrollment(text, text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.set_runner_enrollment(text, text, text, bigint, boolean, text) FROM PUBLIC;

COMMIT;
