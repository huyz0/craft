BEGIN;

CREATE TABLE craft_queue.ci_lanes (
    id text PRIMARY KEY CHECK (id ~ '^[A-Za-z0-9_-]{1,100}$'),
    repository text NOT NULL,
    approval jsonb NOT NULL,
    approved_by text NOT NULL,
    approved_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    disabled_by text,
    disabled_at timestamptz,
    disable_evidence text
);
CREATE UNIQUE INDEX active_ci_lane_per_repository ON craft_queue.ci_lanes(lower(repository)) WHERE disabled_at IS NULL;

CREATE TABLE craft_queue.ci_lane_admissions (
    work_id text PRIMARY KEY REFERENCES craft_queue.work(id),
    lane_id text NOT NULL REFERENCES craft_queue.ci_lanes(id),
    repository text NOT NULL,
    admitted_at timestamptz NOT NULL
);
CREATE INDEX ci_lane_window ON craft_queue.ci_lane_admissions(lower(repository), admitted_at);

CREATE FUNCTION craft_queue.approve_ci_lane(actor text, lane text, proposed jsonb) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    existing craft_queue.ci_lanes%ROWTYPE;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    SELECT * INTO existing FROM craft_queue.ci_lanes AS rules WHERE rules.id = lane;
    IF FOUND THEN
        IF existing.approval <> proposed OR existing.approved_by <> actor OR existing.disabled_at IS NOT NULL
            THEN RETURN NULL; END IF;
    ELSE
        IF EXISTS (SELECT 1 FROM craft_queue.ci_lanes AS rules
            WHERE lower(rules.repository) = lower(proposed->>'repository') AND rules.disabled_at IS NULL)
            OR EXISTS (SELECT 1 FROM craft_queue.recoveries AS r WHERE r.finished_at IS NULL) THEN RETURN NULL; END IF;
        INSERT INTO craft_queue.ci_lanes(id, repository, approval, approved_by)
        VALUES (lane, proposed->>'repository', proposed, actor) RETURNING * INTO existing;
    END IF;
    RETURN to_jsonb(existing) - 'approval' || jsonb_build_object(
        'observer', existing.approval->>'observer', 'manifest_digest', existing.approval->>'manifest_digest',
        'evidence', existing.approval->>'evidence', 'allow_admission', existing.approval->'allow_admission');
END;
$$;

CREATE FUNCTION craft_queue.disable_ci_lane(actor text, lane text, reason text) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    existing craft_queue.ci_lanes%ROWTYPE;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    SELECT * INTO existing FROM craft_queue.ci_lanes AS rules WHERE rules.id = lane;
    IF NOT FOUND THEN RETURN NULL; END IF;
    IF existing.disabled_at IS NOT NULL THEN
        IF existing.disabled_by <> actor OR existing.disable_evidence <> reason THEN RETURN NULL; END IF;
    ELSE
        UPDATE craft_queue.ci_lanes SET disabled_by = actor, disabled_at = clock_timestamp(), disable_evidence = reason
        WHERE id = lane RETURNING * INTO existing;
    END IF;
    RETURN to_jsonb(existing) - 'approval';
END;
$$;

CREATE FUNCTION craft_queue.observe_ci_lane(
    observer text, lane text, manifest_pin text, failure jsonb, failure_digest text, should_admit boolean
) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    settings craft_queue.control%ROWTYPE;
    rule craft_queue.ci_lanes%ROWTYPE;
    template jsonb;
    forge_selector jsonb;
    summary jsonb;
    tokens bigint;
    event_identity text;
    order_id text;
    base_commit text;
    anchor_commit text;
    observed_at timestamptz;
BEGIN
    SELECT * INTO STRICT settings FROM craft_queue.control FOR UPDATE;
    SELECT * INTO rule FROM craft_queue.ci_lanes AS rules WHERE rules.id = lane AND rules.disabled_at IS NULL
        AND rules.approval->>'observer' = observer AND rules.approval->>'manifest_digest' = manifest_pin;
    IF NOT FOUND THEN RETURN NULL; END IF;
    IF failure IS NULL THEN RETURN jsonb_build_object('state', 'no_failure', 'lane', lane); END IF;

    template := rule.approval->'template';
    forge_selector := rule.approval->'selector';
    base_commit := failure->>'commit';
    anchor_commit := template->>'base';
    IF failure->>'repository' IS DISTINCT FROM rule.repository
        OR failure->>'branch' IS DISTINCT FROM rule.approval->>'branch'
        OR length(base_commit) IS DISTINCT FROM length(anchor_commit)
        OR (rule.approval->>'follow_branch' = 'false' AND base_commit IS DISTINCT FROM anchor_commit)
        OR (CASE forge_selector->>'forge'
            WHEN 'github' THEN failure->'workflow_id' IS DISTINCT FROM forge_selector->'workflow_id'
                OR failure->'job_name' IS DISTINCT FROM forge_selector->'job_name'
            WHEN 'bitbucket' THEN failure->'step_name' IS DISTINCT FROM forge_selector->'step_name'
            ELSE true END)
        OR COALESCE(failure_digest, '') !~ '^[a-f0-9]{64}$' THEN RETURN NULL; END IF;

    event_identity := 'ci:' || failure_digest;
    SELECT w.id INTO order_id FROM craft_queue.work AS w
    WHERE w.event_key = event_identity AND w.repository = rule.repository;
    IF FOUND THEN
        RETURN jsonb_build_object('state', 'already_admitted', 'lane', lane, 'work_order_id', order_id);
    END IF;
    IF NOT should_admit OR rule.approval->>'allow_admission' <> 'true' THEN
        RETURN jsonb_build_object('state', 'shadow', 'lane', lane, 'event_key', event_identity);
    END IF;
    IF settings.paused OR EXISTS (SELECT 1 FROM craft_queue.recoveries AS r WHERE r.finished_at IS NULL) THEN
        RETURN jsonb_build_object('state', 'paused', 'lane', lane);
    END IF;
    SELECT w.id INTO order_id FROM craft_queue.work AS w WHERE lower(w.repository) = lower(rule.repository)
        AND w.state IN ('queued', 'leased', 'candidate', 'blocked') AND w.payload->>'ci_failure' IS NOT NULL
        ORDER BY w.created_at, w.id LIMIT 1;
    IF FOUND THEN
        RETURN jsonb_build_object('state', 'lane_occupied', 'lane', lane, 'work_order_id', order_id);
    END IF;
    observed_at := clock_timestamp();
    IF (SELECT count(*) FROM craft_queue.ci_lane_admissions AS history
        WHERE lower(history.repository) = lower(rule.repository) AND history.admitted_at > observed_at - interval '24 hours'
    ) >= (rule.approval->>'daily_limit')::integer THEN
        RETURN jsonb_build_object('state', 'daily_limit', 'lane', lane);
    END IF;
    IF NOT EXISTS (SELECT 1 FROM craft_queue.runner_repositories AS runners WHERE runners.repository = rule.repository) THEN
        RETURN jsonb_build_object('state', 'runner_unavailable', 'lane', lane);
    END IF;
    summary := craft_queue.operation_summary();
    IF (summary->>'review_reserved')::bigint + (
        SELECT count(*) FROM craft_queue.ci_lane_admissions AS history
        JOIN craft_queue.work AS w ON w.id = history.work_id
        WHERE w.state = 'queued' AND w.repair_from_generation IS NULL
    ) >= settings.review_limit THEN
        RETURN jsonb_build_object('state', 'review_capacity', 'lane', lane);
    END IF;
    tokens := (rule.approval->>'token_limit')::bigint;
    IF (summary->>'tokens_charged_or_reserved')::numeric + tokens + (
        SELECT COALESCE(sum(w.reserved_tokens), 0) FROM craft_queue.ci_lane_admissions AS history
        JOIN craft_queue.work AS w ON w.id = history.work_id
        WHERE w.state = 'queued' AND w.generation = 0
    ) > settings.token_budget THEN
        RETURN jsonb_build_object('state', 'token_capacity', 'lane', lane);
    END IF;
    order_id := left(template->>'id', 32) || '-ci-' || failure_digest;
    IF EXISTS (SELECT 1 FROM craft_queue.work AS w WHERE w.id = order_id OR w.event_key = event_identity)
        OR EXISTS (SELECT 1 FROM craft_queue.archived_local_orders AS archived
            WHERE archived.work_id = order_id OR right(archived.work_id, 68) = '-ci-' || failure_digest) THEN
        RETURN jsonb_build_object('state', 'identity_conflict', 'lane', lane);
    END IF;
    PERFORM craft_queue.admit(order_id, rule.repository, event_identity, template || jsonb_build_object(
        'id', order_id, 'base', base_commit, 'ci_failure', failure,
        'ci_policy_digest', rule.approval->>'policy_digest', 'require_native_checks', true)
        || CASE WHEN rule.approval->>'follow_branch' = 'true'
             THEN jsonb_build_object('ci_anchor', anchor_commit) ELSE '{}'::jsonb END, tokens);
    INSERT INTO craft_queue.ci_lane_admissions(work_id, lane_id, repository, admitted_at)
    VALUES (order_id, lane, rule.repository, observed_at);
    RETURN jsonb_build_object('state', 'admitted', 'lane', lane, 'work_order_id', order_id);
END;
$$;

CREATE FUNCTION craft_queue.ci_lane_admission_for(order_id text) RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT jsonb_build_object('lane', rule.id, 'observer', rule.approval->>'observer',
        'manifest_digest', rule.approval->>'manifest_digest', 'policy_digest', rule.approval->>'policy_digest',
        'approved_by', rule.approved_by, 'approval_evidence', rule.approval->>'evidence',
        'admitted_at', history.admitted_at)
    FROM craft_queue.ci_lane_admissions AS history JOIN craft_queue.ci_lanes AS rule ON rule.id = history.lane_id
    WHERE history.work_id = order_id;
$$;

REVOKE ALL ON FUNCTION craft_queue.approve_ci_lane(text,text,jsonb) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.disable_ci_lane(text,text,text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.observe_ci_lane(text,text,text,jsonb,text,boolean) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.ci_lane_admission_for(text) FROM PUBLIC;

COMMIT;
