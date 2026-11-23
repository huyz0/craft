BEGIN;

ALTER TABLE craft_queue.work DROP CONSTRAINT work_state_check;
ALTER TABLE craft_queue.work ADD CONSTRAINT work_state_check
    CHECK (state IN ('queued', 'leased', 'candidate', 'cancelled', 'released'));

CREATE TABLE craft_queue.release_series (
    id text PRIMARY KEY CHECK (id ~ '^[A-Za-z0-9_-]{1,100}$'),
    owner_name text NOT NULL CHECK (length(btrim(owner_name)) BETWEEN 1 AND 200),
    approval_reference text NOT NULL CHECK (length(btrim(approval_reference)) BETWEEN 1 AND 500),
    registered_by text NOT NULL,
    plan_digest text NOT NULL,
    registered_at timestamptz NOT NULL DEFAULT clock_timestamp()
);

CREATE TABLE craft_queue.series_members (
    series_id text NOT NULL REFERENCES craft_queue.release_series(id),
    position integer NOT NULL CHECK (position BETWEEN 1 AND 12),
    work_id text NOT NULL UNIQUE REFERENCES craft_queue.work(id),
    observer_id text NOT NULL CHECK (observer_id ~ '^[A-Za-z0-9_-]{1,100}$'),
    release_target jsonb NOT NULL,
    predecessor_releases jsonb,
    PRIMARY KEY (series_id, position)
);

CREATE TABLE craft_queue.series_observations (
    sequence bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    work_id text NOT NULL REFERENCES craft_queue.series_members(work_id),
    generation bigint NOT NULL,
    head text NOT NULL,
    verification_sequence bigint NOT NULL REFERENCES craft_queue.verifications(sequence),
    observer_id text NOT NULL,
    artifact_digest text NOT NULL CHECK (artifact_digest ~ '^sha256:[a-f0-9]{64}$'),
    evidence text NOT NULL,
    observed_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    withdrawn_at timestamptz,
    withdrawal_evidence text,
    UNIQUE (work_id, evidence)
);
CREATE UNIQUE INDEX series_current_release
    ON craft_queue.series_observations(work_id) WHERE withdrawn_at IS NULL;

CREATE FUNCTION craft_queue.series_release_current(order_id text) RETURNS boolean
LANGUAGE sql STABLE SET search_path = pg_catalog AS $$
    SELECT EXISTS (
        SELECT 1 FROM craft_queue.series_members AS member
        JOIN craft_queue.work AS w ON w.id = member.work_id AND w.state = 'released'
        JOIN craft_queue.series_observations AS observed ON observed.work_id = w.id AND observed.withdrawn_at IS NULL
        WHERE member.work_id = order_id AND NOT EXISTS (
            SELECT 1 FROM jsonb_each(member.predecessor_releases) AS pin
            LEFT JOIN craft_queue.series_observations AS prior ON prior.work_id = pin.key AND prior.withdrawn_at IS NULL
            WHERE prior.sequence IS NULL OR pin.value IS DISTINCT FROM
                jsonb_build_object('head', prior.head, 'artifact_digest', prior.artifact_digest)
        )
    );
$$;

CREATE FUNCTION craft_queue.series_blocker(order_id text) RETURNS text
LANGUAGE sql STABLE SET search_path = pg_catalog AS $$
    SELECT dependency.work_id
    FROM craft_queue.series_members AS member
    JOIN craft_queue.series_members AS dependency ON dependency.series_id = member.series_id
         AND dependency.position < member.position
    LEFT JOIN craft_queue.series_observations AS released ON released.work_id = dependency.work_id
        AND released.withdrawn_at IS NULL
    WHERE member.work_id = order_id AND (
        NOT craft_queue.series_release_current(dependency.work_id) OR (
            member.predecessor_releases IS NOT NULL
            AND member.predecessor_releases->dependency.work_id IS DISTINCT FROM
                jsonb_build_object('head', released.head, 'artifact_digest', released.artifact_digest)
        )
    )
    ORDER BY dependency.position LIMIT 1;
$$;

CREATE FUNCTION craft_queue.series_status(plan_id text) RETURNS jsonb
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT to_jsonb(plan) || jsonb_build_object(
        'state', CASE WHEN summary.released = summary.total THEN 'released'
                      WHEN summary.released > 0 THEN 'partial' ELSE 'open' END,
        'released_members', summary.released, 'total_members', summary.total, 'members', summary.members
    )
    FROM craft_queue.release_series AS plan
    CROSS JOIN LATERAL (
        SELECT count(*) AS total, count(*) FILTER (WHERE craft_queue.series_release_current(w.id)) AS released,
               jsonb_agg(jsonb_build_object(
                   'position', member.position, 'work_id', w.id, 'repository', w.repository,
                   'owner', w.payload->>'owner', 'state', w.state, 'generation', w.generation,
                   'head', w.candidate, 'waiting_on', craft_queue.series_blocker(w.id),
                   'target', member.release_target, 'observer', member.observer_id,
                   'predecessor_releases', member.predecessor_releases,
                   'latest_release', (SELECT to_jsonb(o) FROM craft_queue.series_observations AS o
                        WHERE o.work_id = w.id ORDER BY o.sequence DESC LIMIT 1)
               ) ORDER BY member.position) AS members
        FROM craft_queue.series_members AS member JOIN craft_queue.work AS w ON w.id = member.work_id
        WHERE member.series_id = plan.id
    ) AS summary
    WHERE plan.id = plan_id;
$$;

CREATE FUNCTION craft_queue.register_series(
    actor text, plan_id text, plan_owner text, approval text, members jsonb
) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    fingerprint text;
    existing text;
    item record;
    order_id text;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    IF jsonb_typeof(members) IS DISTINCT FROM 'array' THEN RETURN NULL; END IF;
    IF jsonb_array_length(members) NOT BETWEEN 2 AND 12 OR COALESCE(plan_id, '') !~ '^[A-Za-z0-9_-]{1,100}$'
       OR length(btrim(COALESCE(plan_owner, ''))) NOT BETWEEN 1 AND 200
       OR length(btrim(COALESCE(approval, ''))) NOT BETWEEN 1 AND 500 THEN RETURN NULL; END IF;
    fingerprint := 'sha256:' || encode(sha256(convert_to(
        jsonb_build_array(plan_owner, approval, members)::text, 'UTF8')), 'hex');
    SELECT plan.plan_digest INTO existing FROM craft_queue.release_series AS plan WHERE plan.id = plan_id;
    IF FOUND THEN
        IF existing = fingerprint THEN RETURN craft_queue.series_status(plan_id); END IF;
        RETURN NULL;
    END IF;
    IF (SELECT count(DISTINCT value->>'repository') FROM jsonb_array_elements(members)) < 2 THEN
        RETURN NULL;
    END IF;
    INSERT INTO craft_queue.release_series(id, owner_name, approval_reference, registered_by, plan_digest)
    VALUES (plan_id, plan_owner, approval, actor, fingerprint);
    FOR item IN SELECT value AS member, ordinality AS ordinal FROM jsonb_array_elements(members) WITH ORDINALITY
    LOOP
        order_id := item.member #>> '{work_order,id}';
        IF COALESCE(order_id, '') !~ '^[A-Za-z0-9_-]{1,100}$'
           OR EXISTS (SELECT 1 FROM craft_queue.work AS w WHERE w.id = order_id)
           OR length(btrim(COALESCE(item.member #>> '{work_order,owner}', ''))) NOT BETWEEN 1 AND 200
           OR COALESCE(item.member->>'observer', '') !~ '^[A-Za-z0-9_-]{1,100}$' THEN
            RAISE EXCEPTION 'series requires new work IDs, owners, and release observers';
        END IF;
        PERFORM craft_queue.admit(order_id, item.member->>'repository',
            'series:' || plan_id || ':' || item.ordinal::text, item.member->'work_order',
            (item.member->>'token_limit')::bigint);
        INSERT INTO craft_queue.series_members(series_id, position, work_id, observer_id, release_target)
        VALUES (plan_id, item.ordinal, order_id, item.member->>'observer', item.member->'target');
    END LOOP;
    RETURN craft_queue.series_status(plan_id);
END;
$$;

CREATE FUNCTION craft_queue.record_series_release(
    actor text, order_id text, candidate_generation bigint, candidate_head text,
    review_sequence bigint, observed_target jsonb, digest text, evidence_ref text
) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    member craft_queue.series_members%ROWTYPE;
    observed craft_queue.series_observations%ROWTYPE;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    SELECT * INTO member FROM craft_queue.series_members AS m
    WHERE m.work_id = order_id AND m.observer_id = actor AND m.release_target = observed_target;
    IF NOT FOUND OR length(btrim(COALESCE(evidence_ref, ''))) NOT BETWEEN 1 AND 500
       OR COALESCE(digest, '') !~ '^sha256:[a-f0-9]{64}$' THEN RETURN NULL; END IF;

    SELECT * INTO observed FROM craft_queue.series_observations AS o
    WHERE o.work_id = order_id AND o.evidence = evidence_ref;
    IF FOUND THEN
        IF observed.head = candidate_head AND observed.generation = candidate_generation
           AND observed.verification_sequence = review_sequence AND observed.artifact_digest = digest
           AND observed.withdrawn_at IS NULL THEN RETURN to_jsonb(observed); END IF;
        RETURN NULL;
    END IF;
    IF EXISTS (SELECT 1 FROM craft_queue.recoveries AS recovery WHERE recovery.finished_at IS NULL)
       OR craft_queue.series_blocker(order_id) IS NOT NULL OR NOT EXISTS (
        SELECT 1 FROM craft_queue.work AS w
        JOIN LATERAL (
            SELECT v.* FROM craft_queue.verifications AS v
            WHERE v.work_id = w.id AND v.generation = w.generation ORDER BY v.sequence DESC LIMIT 1
        ) AS latest ON true
        WHERE w.id = order_id AND w.state = 'candidate' AND w.generation = candidate_generation
          AND w.candidate = candidate_head AND latest.sequence = review_sequence AND latest.passed
          AND latest.head = candidate_head AND latest.digest = w.candidate_evidence
          AND craft_queue.team_skill_allowed(w.repository, w.payload)
          AND craft_queue.project_context_allowed(w.repository, w.payload)
    ) OR EXISTS (
        SELECT 1 FROM craft_queue.publications AS p WHERE p.work_id = order_id
          AND p.state IN ('reserved','sending','uncertain')
    ) THEN RETURN NULL; END IF;
    INSERT INTO craft_queue.series_observations(
        work_id, generation, head, verification_sequence, observer_id, artifact_digest, evidence
    ) VALUES (order_id, candidate_generation, candidate_head, review_sequence, actor, digest, evidence_ref)
    RETURNING * INTO observed;
    UPDATE craft_queue.work AS w SET state = 'released' WHERE w.id = order_id;
    RETURN to_jsonb(observed);
END;
$$;

CREATE FUNCTION craft_queue.withdraw_series_release(
    actor text, order_id text, observation_sequence bigint, evidence_ref text
) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    observed craft_queue.series_observations%ROWTYPE;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    SELECT o.* INTO observed FROM craft_queue.series_observations AS o
    JOIN craft_queue.series_members AS member ON member.work_id = o.work_id
    WHERE o.sequence = observation_sequence AND o.work_id = order_id AND member.observer_id = actor;
    IF NOT FOUND OR length(btrim(COALESCE(evidence_ref, ''))) NOT BETWEEN 1 AND 500 THEN RETURN NULL; END IF;
    IF observed.withdrawn_at IS NOT NULL THEN
        IF observed.withdrawal_evidence = evidence_ref THEN RETURN to_jsonb(observed); END IF;
        RETURN NULL;
    END IF;
    UPDATE craft_queue.series_observations AS o SET withdrawn_at = clock_timestamp(), withdrawal_evidence = evidence_ref
    WHERE o.sequence = observation_sequence RETURNING o.* INTO observed;
    UPDATE craft_queue.work AS w
    SET state = CASE WHEN craft_queue.team_skill_allowed(w.repository, w.payload)
        AND craft_queue.project_context_allowed(w.repository, w.payload) THEN 'candidate' ELSE 'cancelled' END,
        cancellation_reason = CASE WHEN craft_queue.team_skill_allowed(w.repository, w.payload)
        AND craft_queue.project_context_allowed(w.repository, w.payload) THEN NULL
        ELSE 'Release withdrawn after required context or skill was retired' END
    WHERE w.id = order_id AND w.state = 'released';
    RETURN to_jsonb(observed);
END;
$$;

CREATE FUNCTION craft_queue.guard_series_worker() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog AS $$
BEGIN
    IF NEW.state = 'leased' THEN
        IF craft_queue.series_blocker(NEW.id) IS NOT NULL THEN
            RAISE EXCEPTION 'series predecessor is unavailable to the worker';
        END IF;
        UPDATE craft_queue.series_members AS member SET predecessor_releases = COALESCE((
            SELECT jsonb_object_agg(dependency.work_id,
                jsonb_build_object('head', released.head, 'artifact_digest', released.artifact_digest))
            FROM craft_queue.series_members AS dependency
            JOIN craft_queue.series_observations AS released ON released.work_id = dependency.work_id
                AND released.withdrawn_at IS NULL
            WHERE dependency.series_id = member.series_id AND dependency.position < member.position
        ), '{}'::jsonb)
        WHERE member.work_id = NEW.id AND member.predecessor_releases IS NULL;
    END IF;
    IF NEW.state = 'candidate' AND OLD.state = 'leased'
       AND craft_queue.series_blocker(NEW.id) IS NOT NULL THEN
        RAISE EXCEPTION 'series predecessor must be released before candidate submission';
    END IF;
    IF OLD.state = 'released' AND NEW.state = 'cancelled' AND EXISTS (
        SELECT 1 FROM craft_queue.series_observations AS o WHERE o.work_id = OLD.id AND o.withdrawn_at IS NULL
    ) THEN
        RAISE EXCEPTION 'withdraw release evidence before cancelling a released member';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER guard_series_worker BEFORE UPDATE OF state ON craft_queue.work
FOR EACH ROW EXECUTE FUNCTION craft_queue.guard_series_worker();

CREATE FUNCTION craft_queue.guard_series_publication() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog AS $$
BEGIN
    IF NEW.state IN ('reserved', 'sending') AND craft_queue.series_blocker(NEW.work_id) IS NOT NULL THEN
        RAISE EXCEPTION 'series predecessor must be released before publication';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER guard_series_publication BEFORE INSERT OR UPDATE OF state ON craft_queue.publications
FOR EACH ROW EXECUTE FUNCTION craft_queue.guard_series_publication();

CREATE FUNCTION craft_queue.withdraw_series_on_restore() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog AS $$
BEGIN
    UPDATE craft_queue.series_observations
    SET withdrawn_at = clock_timestamp(), withdrawal_evidence = 'Restore recovery ' || NEW.epoch::text
    WHERE withdrawn_at IS NULL;
    UPDATE craft_queue.work AS w
    SET state = CASE WHEN craft_queue.team_skill_allowed(w.repository, w.payload)
        AND craft_queue.project_context_allowed(w.repository, w.payload) THEN 'candidate' ELSE 'cancelled' END,
        cancellation_reason = CASE WHEN craft_queue.team_skill_allowed(w.repository, w.payload)
        AND craft_queue.project_context_allowed(w.repository, w.payload) THEN NULL
        ELSE 'Release needs fresh observation after restore and required context or skill was retired' END
    WHERE w.state = 'released';
    RETURN NEW;
END;
$$;

CREATE TRIGGER withdraw_series_on_restore AFTER INSERT ON craft_queue.recoveries
FOR EACH ROW EXECUTE FUNCTION craft_queue.withdraw_series_on_restore();

CREATE OR REPLACE FUNCTION craft_queue.claim(runner text, request uuid, context_capable boolean)
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
          AND craft_queue.standing_runner_allowed(w.id, runner, now_at)
          AND craft_queue.series_blocker(w.id) IS NULL
          AND (context_capable OR NOT EXISTS (SELECT 1 FROM craft_queue.project_context_pins(w.payload)));
        RETURN;
    END IF;
    IF settings.paused THEN RETURN; END IF;

    SELECT COUNT(*) < settings.review_limit INTO review_free
    FROM craft_queue.work AS w
    LEFT JOIN craft_queue.standing_work AS standing ON standing.work_id = w.id
    WHERE w.state IN ('leased', 'candidate')
      OR (w.state = 'queued' AND (w.repair_from_generation IS NOT NULL OR standing.work_id IS NOT NULL));
    SELECT settings.token_budget - COALESCE(SUM(COALESCE(a.reported_tokens, a.token_limit)), 0)
    INTO tokens_free FROM craft_queue.attempts AS a
    WHERE a.started_at > now_at - interval '24 hours'
       OR (a.outcome = 'leased' AND a.expires_at > now_at);

    SELECT w.* INTO selected
    FROM craft_queue.work AS w
    JOIN craft_queue.runner_repositories AS grant_row ON grant_row.repository = w.repository
    LEFT JOIN craft_queue.attempts AS a ON a.work_id = w.id AND a.generation = w.generation
    LEFT JOIN craft_queue.standing_work AS standing ON standing.work_id = w.id
    WHERE grant_row.runner_id = runner AND w.reserved_tokens <= tokens_free
      AND craft_queue.standing_runner_allowed(w.id, runner, now_at)
      AND craft_queue.series_blocker(w.id) IS NULL
      AND (context_capable OR NOT EXISTS (SELECT 1 FROM craft_queue.project_context_pins(w.payload)))
      AND ((w.state = 'queued' AND (review_free OR w.repair_from_generation IS NOT NULL OR standing.work_id IS NOT NULL))
        OR (w.state = 'leased' AND a.outcome = 'leased' AND a.expires_at <= now_at))
    ORDER BY (w.state = 'leased' OR w.repair_from_generation IS NOT NULL OR standing.work_id IS NOT NULL) DESC,
        w.created_at, w.id
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

CREATE OR REPLACE FUNCTION craft_queue.renew(runner text, lease_work_id text, lease_generation bigint)
RETURNS timestamptz
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    settings craft_queue.control%ROWTYPE;
    now_at timestamptz;
    renewed timestamptz;
BEGIN
    SELECT * INTO STRICT settings FROM craft_queue.control FOR UPDATE;
    now_at := clock_timestamp();
    UPDATE craft_queue.attempts AS a
    SET expires_at = now_at + make_interval(secs => settings.lease_seconds)
    FROM craft_queue.work AS w, craft_queue.runner_repositories AS grant_row
    WHERE w.id = lease_work_id AND w.state = 'leased' AND w.generation = lease_generation
      AND a.work_id = w.id AND a.generation = w.generation AND a.runner_id = runner
      AND a.outcome = 'leased' AND a.expires_at > now_at
      AND grant_row.runner_id = runner AND grant_row.repository = w.repository
      AND craft_queue.series_blocker(w.id) IS NULL
    RETURNING a.expires_at INTO renewed;
    RETURN renewed;
END;
$$;

CREATE OR REPLACE FUNCTION craft_queue.activate_cutover(transfer uuid, receipt_archive text, operator_id text)
RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    settings craft_queue.control%ROWTYPE;
    existing craft_queue.cutovers%ROWTYPE;
    archive_json jsonb;
BEGIN
    SELECT * INTO STRICT settings FROM craft_queue.control FOR UPDATE;
    SELECT * INTO existing FROM craft_queue.cutovers AS c WHERE c.epoch = transfer;
    IF FOUND THEN
        IF existing.archive <> receipt_archive THEN RAISE EXCEPTION 'cutover archive differs'; END IF;
        RETURN craft_queue.cutover_status(transfer);
    END IF;
    IF NOT settings.paused
        OR EXISTS (SELECT 1 FROM craft_queue.work AS w WHERE w.state NOT IN ('cancelled', 'released'))
        OR EXISTS (SELECT 1 FROM craft_queue.cutovers AS c WHERE c.phase <> 'released') THEN
        RAISE EXCEPTION 'pause and drain the shared queue before activation';
    END IF;
    IF receipt_archive IS NULL OR octet_length(receipt_archive) > 524288 THEN
        RAISE EXCEPTION 'invalid local receipt archive size';
    END IF;
    archive_json := receipt_archive::jsonb;
    IF archive_json->'schema_version' IS DISTINCT FROM '1'::jsonb
        OR archive_json->>'epoch' IS DISTINCT FROM transfer::text
        OR jsonb_typeof(archive_json->'receipts') IS DISTINCT FROM 'array'
        OR EXISTS (
            SELECT 1 FROM jsonb_array_elements(archive_json->'receipts') AS r(receipt)
            WHERE COALESCE(r.receipt->>'state','') NOT IN ('closed', 'cancelled')
               OR COALESCE(r.receipt->'work_order'->>'id','') !~ '^[A-Za-z0-9_-]{1,100}$'
        ) THEN
        RAISE EXCEPTION 'archive must contain terminal receipts for this epoch';
    END IF;
    INSERT INTO craft_queue.cutovers (epoch, phase, archive, digest, actor)
    VALUES (transfer, 'shared', receipt_archive, 'sha256:' || encode(sha256(convert_to(receipt_archive, 'UTF8')), 'hex'), operator_id);
    INSERT INTO craft_queue.archived_local_orders (work_id, epoch)
    SELECT r.receipt->'work_order'->>'id', transfer
    FROM jsonb_array_elements(archive_json->'receipts') AS r(receipt)
    ON CONFLICT (work_id) DO NOTHING;
    UPDATE craft_queue.control SET paused = false WHERE singleton;
    RETURN craft_queue.cutover_status(transfer);
END;
$$;

CREATE OR REPLACE FUNCTION craft_queue.release_cutover(transfer uuid, operator_id text) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    current_phase text;
    newest uuid;
    remaining_work boolean;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    SELECT c.phase INTO current_phase FROM craft_queue.cutovers AS c WHERE c.epoch = transfer;
    IF NOT FOUND THEN RETURN NULL; END IF;
    IF current_phase = 'released' THEN RETURN craft_queue.cutover_status(transfer); END IF;
    SELECT c.epoch INTO newest FROM craft_queue.cutovers AS c ORDER BY c.sequence DESC LIMIT 1;
    IF transfer <> newest THEN RETURN NULL; END IF;

    SELECT EXISTS (SELECT 1 FROM craft_queue.work AS w WHERE w.state NOT IN ('cancelled', 'released')) INTO remaining_work;
    UPDATE craft_queue.control SET paused = true WHERE singleton;
    UPDATE craft_queue.cutovers AS c
    SET phase = CASE WHEN remaining_work THEN 'draining' ELSE 'released' END,
        returned_work = CASE WHEN remaining_work THEN '[]'::jsonb ELSE (
            SELECT COALESCE(jsonb_agg(w.id ORDER BY w.id), '[]'::jsonb) FROM craft_queue.work AS w
        ) END,
        actor = operator_id, changed_at = clock_timestamp()
    WHERE c.epoch = transfer;
    RETURN craft_queue.cutover_status(transfer);
END;
$$;

REVOKE ALL ON FUNCTION craft_queue.series_blocker(text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.series_release_current(text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.series_status(text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.register_series(text, text, text, text, jsonb) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.record_series_release(text, text, bigint, text, bigint, jsonb, text, text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.withdraw_series_release(text, text, bigint, text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.guard_series_worker() FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.guard_series_publication() FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.withdraw_series_on_restore() FROM PUBLIC;
COMMIT;
