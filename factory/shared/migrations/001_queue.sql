BEGIN;

CREATE SCHEMA craft_queue;
REVOKE ALL ON SCHEMA craft_queue FROM PUBLIC;

CREATE TABLE craft_queue.control (
    singleton boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    paused boolean NOT NULL DEFAULT true,
    review_limit integer NOT NULL DEFAULT 1 CHECK (review_limit > 0),
    token_budget bigint NOT NULL DEFAULT 100000 CHECK (token_budget > 0),
    lease_seconds integer NOT NULL DEFAULT 120 CHECK (lease_seconds BETWEEN 1 AND 900)
);
INSERT INTO craft_queue.control DEFAULT VALUES;

CREATE TABLE craft_queue.runner_repositories (
    runner_id text NOT NULL CHECK (length(runner_id) BETWEEN 1 AND 100),
    repository text NOT NULL CHECK (length(repository) BETWEEN 1 AND 200),
    PRIMARY KEY (runner_id, repository)
);

CREATE TABLE craft_queue.work (
    id text PRIMARY KEY CHECK (id ~ '^[A-Za-z0-9_-]{1,100}$'),
    repository text NOT NULL CHECK (length(repository) BETWEEN 1 AND 200),
    event_key text NOT NULL UNIQUE CHECK (length(event_key) BETWEEN 1 AND 300),
    payload jsonb NOT NULL,
    reserved_tokens bigint NOT NULL CHECK (reserved_tokens > 0),
    state text NOT NULL DEFAULT 'queued' CHECK (state IN ('queued', 'leased', 'candidate', 'cancelled')),
    generation bigint NOT NULL DEFAULT 0,
    candidate text,
    candidate_evidence text,
    cancellation_reason text,
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    CHECK (jsonb_typeof(payload) = 'object'),
    CHECK (candidate IS NULL OR candidate ~ '^([a-f0-9]{40}|[a-f0-9]{64})$')
);

CREATE TABLE craft_queue.attempts (
    work_id text NOT NULL REFERENCES craft_queue.work(id),
    generation bigint NOT NULL,
    runner_id text NOT NULL,
    request_id uuid NOT NULL,
    expires_at timestamptz NOT NULL,
    started_at timestamptz NOT NULL,
    token_limit bigint NOT NULL CHECK (token_limit > 0),
    reported_tokens bigint CHECK (reported_tokens BETWEEN 0 AND token_limit),
    outcome text NOT NULL DEFAULT 'leased' CHECK (outcome IN ('leased', 'expired', 'candidate', 'cancelled')),
    PRIMARY KEY (work_id, generation),
    UNIQUE (runner_id, request_id)
);
CREATE INDEX attempts_started_at ON craft_queue.attempts (started_at);

CREATE FUNCTION craft_queue.admit(
    work_id text, repo text, event_identity text, work_order jsonb, token_limit bigint
) RETURNS text
LANGUAGE plpgsql SET search_path = pg_catalog AS $$
DECLARE
    existing craft_queue.work%ROWTYPE;
    settings craft_queue.control%ROWTYPE;
BEGIN
    SELECT * INTO STRICT settings FROM craft_queue.control FOR UPDATE;
    SELECT * INTO existing FROM craft_queue.work AS w
    WHERE w.id = work_id OR w.event_key = event_identity
    ORDER BY (w.id = work_id) DESC LIMIT 1;
    IF FOUND THEN
        IF existing.id = work_id AND existing.event_key = event_identity
            AND existing.repository = repo AND existing.payload = work_order
            AND existing.reserved_tokens = token_limit THEN
            RETURN existing.id;
        END IF;
        RAISE EXCEPTION 'work or event identity already has different inputs';
    END IF;
    IF settings.paused THEN
        RAISE EXCEPTION 'shared queue is paused';
    END IF;
    IF work_order->'schema_version' IS DISTINCT FROM '1'::jsonb
        OR work_order->>'id' IS DISTINCT FROM work_id
        OR COALESCE(work_order->>'base', '') !~ '^([a-f0-9]{40}|[a-f0-9]{64})$'
        OR token_limit IS NULL OR token_limit NOT BETWEEN 1 AND settings.token_budget THEN
        RAISE EXCEPTION 'invalid work order identity, base, or token reservation';
    END IF;
    INSERT INTO craft_queue.work (id, repository, event_key, payload, reserved_tokens)
    VALUES (work_id, repo, event_identity, work_order, token_limit);
    RETURN work_id;
END;
$$;

CREATE FUNCTION craft_queue.claim(runner text, request uuid)
RETURNS TABLE (work_id text, repository text, generation bigint, expires_at timestamptz, work_order jsonb)
LANGUAGE plpgsql SET search_path = pg_catalog AS $$
DECLARE
    settings craft_queue.control%ROWTYPE;
    selected craft_queue.work%ROWTYPE;
    previous craft_queue.attempts%ROWTYPE;
    now_at timestamptz;
    review_free boolean;
    tokens_free numeric;
    deadline timestamptz;
BEGIN
    -- Serialize capacity accounting and generation changes across the entire queue.
    SELECT * INTO STRICT settings FROM craft_queue.control FOR UPDATE;
    now_at := clock_timestamp();
    SELECT * INTO previous FROM craft_queue.attempts AS a
    WHERE a.runner_id = runner AND a.request_id = request;
    IF FOUND THEN
        RETURN QUERY SELECT w.id, w.repository, w.generation, previous.expires_at, w.payload - 'repository'
        FROM craft_queue.work AS w
        JOIN craft_queue.runner_repositories AS grant_row ON grant_row.repository = w.repository
        WHERE w.id = previous.work_id AND w.generation = previous.generation
          AND w.state = 'leased' AND previous.outcome = 'leased' AND previous.expires_at > now_at
          AND grant_row.runner_id = runner;
        RETURN;
    END IF;
    IF settings.paused THEN RETURN; END IF;

    SELECT COUNT(*) < settings.review_limit INTO review_free
    FROM craft_queue.work AS w WHERE w.state IN ('leased', 'candidate');
    SELECT settings.token_budget - COALESCE(SUM(COALESCE(a.reported_tokens, a.token_limit)), 0)
    INTO tokens_free FROM craft_queue.attempts AS a
    WHERE a.started_at > now_at - interval '24 hours'
       OR (a.outcome = 'leased' AND a.expires_at > now_at);

    SELECT w.* INTO selected
    FROM craft_queue.work AS w
    JOIN craft_queue.runner_repositories AS grant_row ON grant_row.repository = w.repository
    LEFT JOIN craft_queue.attempts AS a ON a.work_id = w.id AND a.generation = w.generation
    WHERE grant_row.runner_id = runner AND w.reserved_tokens <= tokens_free
      AND ((w.state = 'queued' AND review_free)
        OR (w.state = 'leased' AND a.outcome = 'leased' AND a.expires_at <= now_at))
    ORDER BY (w.state = 'leased') DESC, w.created_at, w.id
    LIMIT 1;
    IF NOT FOUND THEN RETURN; END IF;

    UPDATE craft_queue.attempts AS a SET outcome = 'expired'
    WHERE a.work_id = selected.id AND a.generation = selected.generation AND a.outcome = 'leased';
    UPDATE craft_queue.work AS w SET state = 'leased', generation = w.generation + 1
    WHERE w.id = selected.id RETURNING w.* INTO selected;
    deadline := now_at + make_interval(secs => settings.lease_seconds);
    INSERT INTO craft_queue.attempts (work_id, generation, runner_id, request_id, expires_at, started_at, token_limit)
    VALUES (selected.id, selected.generation, runner, request, deadline, now_at, selected.reserved_tokens);
    RETURN QUERY SELECT selected.id, selected.repository, selected.generation, deadline, selected.payload - 'repository';
END;
$$;

CREATE FUNCTION craft_queue.renew(runner text, lease_work_id text, lease_generation bigint)
RETURNS timestamptz
LANGUAGE plpgsql SET search_path = pg_catalog AS $$
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
    RETURNING a.expires_at INTO renewed;
    RETURN renewed;
END;
$$;

CREATE FUNCTION craft_queue.submit_candidate(
    runner text, lease_work_id text, lease_generation bigint,
    commit_id text, evidence_ref text, tokens bigint
) RETURNS boolean
LANGUAGE plpgsql SET search_path = pg_catalog AS $$
DECLARE
    now_at timestamptz;
    selected craft_queue.work%ROWTYPE;
    attempt craft_queue.attempts%ROWTYPE;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    now_at := clock_timestamp();
    IF COALESCE(commit_id, '') !~ '^([a-f0-9]{40}|[a-f0-9]{64})$'
        OR length(COALESCE(evidence_ref, '')) NOT BETWEEN 1 AND 500 OR tokens < 0 THEN
        RAISE EXCEPTION 'invalid candidate, evidence reference, or usage';
    END IF;
    SELECT w.* INTO selected FROM craft_queue.work AS w
    JOIN craft_queue.runner_repositories AS grant_row ON grant_row.repository = w.repository
    WHERE w.id = lease_work_id AND w.generation = lease_generation AND grant_row.runner_id = runner;
    IF NOT FOUND THEN RETURN false; END IF;
    SELECT * INTO attempt FROM craft_queue.attempts AS a
    WHERE a.work_id = selected.id AND a.generation = selected.generation AND a.runner_id = runner;
    IF NOT FOUND OR tokens > attempt.token_limit THEN RETURN false; END IF;
    IF selected.state = 'candidate' AND attempt.outcome = 'candidate' THEN
        RETURN selected.candidate = commit_id AND selected.candidate_evidence = evidence_ref
             AND attempt.reported_tokens IS NOT DISTINCT FROM tokens;
    END IF;
    IF selected.state <> 'leased' OR attempt.outcome <> 'leased' OR attempt.expires_at <= now_at THEN
        RETURN false;
    END IF;
    UPDATE craft_queue.attempts AS a SET outcome = 'candidate', reported_tokens = tokens
    WHERE a.work_id = selected.id AND a.generation = selected.generation;
    UPDATE craft_queue.work AS w SET state = 'candidate', candidate = commit_id, candidate_evidence = evidence_ref
    WHERE w.id = selected.id;
    RETURN true;
END;
$$;

CREATE FUNCTION craft_queue.cancel(work_id text, reason text) RETURNS boolean
LANGUAGE plpgsql SET search_path = pg_catalog AS $$
DECLARE
    selected craft_queue.work%ROWTYPE;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    IF length(btrim(COALESCE(reason, ''))) NOT BETWEEN 1 AND 500 THEN
        RAISE EXCEPTION 'a cancellation reason is required';
    END IF;
    SELECT * INTO selected FROM craft_queue.work AS w WHERE w.id = work_id;
    IF NOT FOUND THEN RETURN false; END IF;
    IF selected.state = 'cancelled' THEN RETURN selected.cancellation_reason = reason; END IF;
    UPDATE craft_queue.attempts AS a SET outcome = 'cancelled'
    WHERE a.work_id = selected.id AND a.generation = selected.generation AND a.outcome = 'leased';
    UPDATE craft_queue.work AS w SET state = 'cancelled', cancellation_reason = reason WHERE w.id = selected.id;
    RETURN true;
END;
$$;

REVOKE ALL ON ALL FUNCTIONS IN SCHEMA craft_queue FROM PUBLIC;
COMMIT;
