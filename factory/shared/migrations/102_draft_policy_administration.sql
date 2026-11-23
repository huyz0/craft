BEGIN;

ALTER TABLE craft_queue.draft_policies ADD COLUMN revision bigint NOT NULL DEFAULT 1;

CREATE TABLE craft_queue.draft_policy_events (
    repository text NOT NULL REFERENCES craft_queue.draft_policies(repository),
    revision bigint NOT NULL,
    actor text NOT NULL,
    enabled boolean NOT NULL,
    evidence text NOT NULL,
    recorded_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (repository, revision)
);

CREATE FUNCTION craft_queue.advance_draft_policy_revision() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog AS $$
BEGIN
    NEW.revision := OLD.revision + 1;
    RETURN NEW;
END;
$$;

CREATE TRIGGER draft_policy_revision BEFORE UPDATE OF enabled, forge, base_branch, publisher_id
    ON craft_queue.draft_policies FOR EACH ROW
    WHEN ((OLD.enabled, OLD.forge, OLD.base_branch, OLD.publisher_id) IS DISTINCT FROM
          (NEW.enabled, NEW.forge, NEW.base_branch, NEW.publisher_id))
    EXECUTE FUNCTION craft_queue.advance_draft_policy_revision();

CREATE FUNCTION craft_queue.inspect_draft_policy(repo text) RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT to_jsonb(p) || jsonb_build_object('decision', to_jsonb(e) - 'repository' - 'revision' - 'enabled')
    FROM craft_queue.draft_policies AS p
    LEFT JOIN craft_queue.draft_policy_events AS e USING (repository, revision)
    WHERE p.repository = repo;
$$;

CREATE FUNCTION craft_queue.configure_draft_policy(
    actor_id text, repo text, target_branch text, publisher text, provider text,
    expected_revision bigint, active boolean, evidence_ref text
) RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    saved craft_queue.draft_policies%ROWTYPE;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    SELECT p.* INTO saved FROM craft_queue.draft_policies AS p WHERE p.repository = repo FOR UPDATE;
    IF FOUND THEN
        IF (saved.base_branch, saved.publisher_id, saved.forge) IS DISTINCT FROM
           (target_branch, publisher, provider) THEN RETURN NULL; END IF;
        IF saved.revision = expected_revision + 1 AND saved.enabled = active AND EXISTS (
            SELECT 1 FROM craft_queue.draft_policy_events AS e
            WHERE e.repository = repo AND e.revision = saved.revision
                AND e.actor = actor_id AND e.evidence = evidence_ref
        ) THEN RETURN craft_queue.inspect_draft_policy(repo); END IF;
        IF saved.revision <> expected_revision OR saved.enabled = active THEN RETURN NULL; END IF;
    ELSIF expected_revision <> 0 THEN
        RETURN NULL;
    END IF;

    IF active AND (
        EXISTS (SELECT 1 FROM craft_queue.recoveries WHERE finished_at IS NULL)
        OR EXISTS (SELECT 1 FROM craft_queue.publications AS p
            WHERE p.repository = repo AND p.state IN ('reserved', 'sending', 'uncertain'))
    ) THEN RETURN NULL; END IF;

    IF saved.revision IS NULL THEN
        INSERT INTO craft_queue.draft_policies (repository, base_branch, publisher_id, forge, enabled)
        VALUES (repo, target_branch, publisher, provider, active)
        ON CONFLICT (repository) DO NOTHING
        RETURNING * INTO saved;
        IF NOT FOUND THEN RETURN NULL; END IF;
    ELSE
        UPDATE craft_queue.draft_policies AS p SET enabled = active
        WHERE p.repository = repo RETURNING * INTO saved;
    END IF;
    INSERT INTO craft_queue.draft_policy_events (repository, revision, actor, enabled, evidence)
    VALUES (repo, saved.revision, actor_id, active, evidence_ref);

    IF NOT active THEN
        WITH held AS (
            UPDATE craft_queue.publications AS p SET state = 'uncertain'
            WHERE p.repository = repo AND p.state = 'reserved' RETURNING p.action_key
        )
        INSERT INTO craft_queue.publication_events (action_key, state, actor, evidence)
        SELECT action_key, 'uncertain', actor_id, evidence_ref FROM held;
    END IF;
    RETURN craft_queue.inspect_draft_policy(repo);
END;
$$;

REVOKE ALL ON FUNCTION craft_queue.advance_draft_policy_revision() FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.inspect_draft_policy(text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.configure_draft_policy(text,text,text,text,text,bigint,boolean,text) FROM PUBLIC;

COMMIT;
