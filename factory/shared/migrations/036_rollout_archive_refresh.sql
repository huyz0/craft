BEGIN;

CREATE TABLE craft_queue.series_rollout_refreshes (
    id text PRIMARY KEY CHECK (id ~ '^[A-Za-z0-9_-]{1,100}$'),
    observation_sequence bigint NOT NULL REFERENCES craft_queue.series_observations(sequence),
    archive_index_digest text NOT NULL CHECK (archive_index_digest ~ '^sha256:[a-f0-9]{64}$'),
    approved_by text NOT NULL,
    approval_reference text NOT NULL CHECK (length(btrim(approval_reference)) BETWEEN 1 AND 500),
    approved_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    verified_at timestamptz,
    rollout_fresh_through bigint CHECK (rollout_fresh_through > 0),
    CHECK ((verified_at IS NULL) = (rollout_fresh_through IS NULL)),
    UNIQUE (observation_sequence, archive_index_digest)
);

CREATE FUNCTION craft_queue.series_observation_fresh_through(observation_sequence bigint, original_deadline bigint)
RETURNS bigint LANGUAGE sql STABLE SET search_path = pg_catalog AS $$
    SELECT GREATEST(original_deadline, max(refresh.rollout_fresh_through))
    FROM craft_queue.series_rollout_refreshes AS refresh WHERE refresh.observation_sequence = $1;
$$;

CREATE OR REPLACE FUNCTION craft_queue.series_release_current(order_id text) RETURNS boolean
LANGUAGE sql STABLE SET search_path = pg_catalog AS $$
    WITH cutoff AS (SELECT floor(extract(epoch FROM statement_timestamp()))::bigint AS seconds)
    SELECT EXISTS (
        SELECT 1 FROM craft_queue.series_members AS member
        JOIN craft_queue.work AS w ON w.id = member.work_id AND w.state = 'released'
        JOIN craft_queue.series_observations AS observed ON observed.work_id = w.id AND observed.withdrawn_at IS NULL
        CROSS JOIN cutoff
        WHERE member.work_id = order_id
          AND (observed.rollout_fresh_through IS NULL
               OR craft_queue.series_observation_fresh_through(observed.sequence, observed.rollout_fresh_through) >= cutoff.seconds)
          AND NOT EXISTS (
            SELECT 1 FROM jsonb_each(member.predecessor_releases) AS pin
            LEFT JOIN craft_queue.series_observations AS prior ON prior.work_id = pin.key AND prior.withdrawn_at IS NULL
            WHERE prior.sequence IS NULL
                OR craft_queue.series_observation_fresh_through(prior.sequence, prior.rollout_fresh_through) < cutoff.seconds
                OR pin.value IS DISTINCT FROM jsonb_build_object('head', prior.head, 'artifact_digest', prior.artifact_digest)
        )
    );
$$;

CREATE FUNCTION craft_queue.approve_rollout_refresh(
    operator_id text, order_id text, refresh_id text, prior_sequence bigint, expected_policy_digest text,
    index_digest text, approval_ref text
) RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    approved craft_queue.series_rollout_refreshes%ROWTYPE;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    IF COALESCE(refresh_id, '') !~ '^[A-Za-z0-9_-]{1,100}$'
       OR COALESCE(index_digest, '') !~ '^sha256:[a-f0-9]{64}$'
       OR length(btrim(COALESCE(approval_ref, ''))) NOT BETWEEN 1 AND 500 THEN RETURN NULL; END IF;
    IF NOT EXISTS (
        SELECT 1 FROM craft_queue.series_observations AS observed
        JOIN craft_queue.work AS w ON w.id = observed.work_id AND w.state = 'released'
        JOIN craft_queue.series_rollout_approvals AS approval ON approval.work_id = w.id
             AND approval.generation = observed.generation AND approval.verification_sequence = observed.verification_sequence
        WHERE w.id = order_id AND observed.sequence = prior_sequence AND observed.withdrawn_at IS NULL
          AND observed.generation = w.generation AND observed.head = w.candidate
          AND observed.rollout_fresh_through IS NOT NULL AND observed.rollout_policy_digest = expected_policy_digest
          AND approval.archive_index_digest = observed.rollout_evidence_digest AND approval.archive_index_digest <> index_digest
    ) OR craft_queue.series_blocker(order_id) IS NOT NULL THEN RETURN NULL; END IF;
    SELECT * INTO approved FROM craft_queue.series_rollout_refreshes AS refresh
    WHERE refresh.id = refresh_id OR (refresh.observation_sequence = prior_sequence AND refresh.archive_index_digest = index_digest);
    IF FOUND THEN
        IF approved.id = refresh_id AND approved.observation_sequence = prior_sequence
           AND approved.archive_index_digest = index_digest AND approved.approved_by = operator_id
           AND approved.approval_reference = approval_ref THEN RETURN to_jsonb(approved); END IF;
        RETURN NULL;
    END IF;
    INSERT INTO craft_queue.series_rollout_refreshes (id, observation_sequence, archive_index_digest, approved_by, approval_reference)
    VALUES (refresh_id, prior_sequence, index_digest, operator_id, approval_ref) RETURNING * INTO approved;
    RETURN to_jsonb(approved);
END;
$$;

CREATE FUNCTION craft_queue.series_rollout_refresh_binding(
    actor_id text, order_id text, candidate_generation bigint, candidate_head text, review_sequence bigint, refresh_id text
) RETURNS jsonb LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT craft_queue.series_rollout_binding(actor_id, order_id, candidate_generation, candidate_head, review_sequence)
           || jsonb_build_object('archive_index_digest', refresh.archive_index_digest, 'artifact_digest', observed.artifact_digest)
    FROM craft_queue.series_rollout_refreshes AS refresh
    JOIN craft_queue.series_observations AS observed ON observed.sequence = refresh.observation_sequence
    JOIN craft_queue.work AS w ON w.id = observed.work_id AND w.state = 'released'
    WHERE refresh.id = refresh_id AND observed.work_id = order_id AND observed.withdrawn_at IS NULL
      AND observed.generation = candidate_generation AND observed.head = candidate_head
      AND observed.verification_sequence = review_sequence AND observed.observer_id = actor_id;
$$;

CREATE FUNCTION craft_queue.record_rollout_refresh(
    actor_id text, order_id text, candidate_generation bigint, candidate_head text, review_sequence bigint,
    refresh_id text, policy_digest text, index_digest text, artifact_digest text, fresh_through bigint
) RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    binding jsonb;
    approved craft_queue.series_rollout_refreshes%ROWTYPE;
    prior_deadline bigint;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    binding := craft_queue.series_rollout_refresh_binding(actor_id, order_id, candidate_generation, candidate_head, review_sequence, refresh_id);
    IF binding IS NULL OR binding->>'policy_digest' IS DISTINCT FROM policy_digest
       OR binding->>'archive_index_digest' IS DISTINCT FROM index_digest
       OR binding->>'artifact_digest' IS DISTINCT FROM artifact_digest
       OR fresh_through IS NULL OR floor(extract(epoch FROM clock_timestamp()))::bigint > fresh_through
       OR craft_queue.series_blocker(order_id) IS NOT NULL THEN RETURN NULL; END IF;
    SELECT * INTO STRICT approved FROM craft_queue.series_rollout_refreshes AS refresh WHERE refresh.id = refresh_id;
    IF approved.verified_at IS NOT NULL THEN
        IF approved.rollout_fresh_through = fresh_through THEN RETURN to_jsonb(approved); END IF;
        RETURN NULL;
    END IF;
    SELECT craft_queue.series_observation_fresh_through(observed.sequence, observed.rollout_fresh_through)
    INTO STRICT prior_deadline FROM craft_queue.series_observations AS observed WHERE observed.sequence = approved.observation_sequence;
    IF fresh_through <= prior_deadline THEN RETURN NULL; END IF;
    UPDATE craft_queue.series_rollout_refreshes AS refresh
    SET verified_at = clock_timestamp(), rollout_fresh_through = fresh_through
    WHERE refresh.id = refresh_id RETURNING * INTO approved;
    RETURN to_jsonb(approved);
END;
$$;

ALTER FUNCTION craft_queue.series_status(text) RENAME TO series_status_base;
CREATE FUNCTION craft_queue.series_status(plan_id text) RETURNS jsonb
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT jsonb_set(plan.body, '{members}', COALESCE((
        SELECT jsonb_agg(member.value || jsonb_build_object('latest_rollout_refresh', (
            SELECT to_jsonb(refresh) FROM craft_queue.series_rollout_refreshes AS refresh
            WHERE refresh.observation_sequence = (member.value #>> '{latest_release,sequence}')::bigint
              AND refresh.verified_at IS NOT NULL ORDER BY refresh.rollout_fresh_through DESC LIMIT 1
        )) ORDER BY member.ordinality)
        FROM jsonb_array_elements(plan.body->'members') WITH ORDINALITY AS member
    ), '[]'::jsonb))
    FROM (SELECT craft_queue.series_status_base(plan_id) AS body) AS plan;
$$;

REVOKE ALL ON FUNCTION craft_queue.series_observation_fresh_through(bigint, bigint) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.approve_rollout_refresh(text, text, text, bigint, text, text, text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.series_rollout_refresh_binding(text, text, bigint, text, bigint, text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.record_rollout_refresh(text, text, bigint, text, bigint, text, text, text, text, bigint) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.series_status(text) FROM PUBLIC;

COMMIT;
