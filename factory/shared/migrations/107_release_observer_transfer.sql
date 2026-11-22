BEGIN;

ALTER TABLE craft_queue.series_members ADD COLUMN observer_revision bigint NOT NULL DEFAULT 0;
ALTER TABLE craft_queue.series_observations ADD COLUMN withdrawn_by text;
ALTER TABLE craft_queue.series_rollout_refreshes ADD COLUMN verified_by text;

CREATE TABLE craft_queue.series_observer_transfers (
    id text PRIMARY KEY CHECK (id ~ '^[A-Za-z0-9_-]{1,100}$'),
    work_id text NOT NULL REFERENCES craft_queue.series_members(work_id),
    revision bigint NOT NULL,
    previous_observer text NOT NULL,
    next_observer text NOT NULL,
    transferred_by text NOT NULL,
    evidence text NOT NULL,
    transferred_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    UNIQUE (work_id, revision)
);

CREATE FUNCTION craft_queue.transfer_release_observer(
    actor_id text, transfer_id text, order_id text, expected_revision bigint, replacement text, evidence_ref text
) RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    member craft_queue.series_members%ROWTYPE;
    saved craft_queue.series_observer_transfers%ROWTYPE;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    SELECT e.* INTO saved FROM craft_queue.series_observer_transfers AS e WHERE e.id = transfer_id;
    IF FOUND THEN
        IF saved.work_id = order_id AND saved.revision = expected_revision + 1
            AND saved.next_observer = replacement AND saved.transferred_by = actor_id
            AND saved.evidence = evidence_ref THEN RETURN to_jsonb(saved); END IF;
        RETURN NULL;
    END IF;
    SELECT m.* INTO member
    FROM craft_queue.series_members AS m JOIN craft_queue.work AS w ON w.id = m.work_id
    WHERE m.work_id = order_id AND w.state <> 'cancelled';
    IF NOT FOUND OR member.observer_revision <> expected_revision OR member.observer_id = replacement
        OR EXISTS (SELECT 1 FROM craft_queue.recoveries WHERE finished_at IS NULL) THEN RETURN NULL; END IF;

    UPDATE craft_queue.series_members AS m
    SET observer_id = replacement, observer_revision = expected_revision + 1 WHERE m.work_id = order_id;
    INSERT INTO craft_queue.series_observer_transfers
        (id, work_id, revision, previous_observer, next_observer, transferred_by, evidence)
    VALUES (transfer_id, order_id, expected_revision + 1, member.observer_id, replacement, actor_id, evidence_ref)
    RETURNING * INTO saved;
    RETURN to_jsonb(saved);
END;
$$;

CREATE OR REPLACE FUNCTION craft_queue.series_rollout_refresh_binding(
    actor_id text, order_id text, candidate_generation bigint, candidate_head text, review_sequence bigint, refresh_id text
) RETURNS jsonb LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT craft_queue.series_rollout_binding(actor_id, order_id, candidate_generation, candidate_head, review_sequence)
           || jsonb_build_object('archive_index_digest', refresh.archive_index_digest, 'artifact_digest', observed.artifact_digest)
    FROM craft_queue.series_rollout_refreshes AS refresh
    JOIN craft_queue.series_observations AS observed ON observed.sequence = refresh.observation_sequence
    JOIN craft_queue.work AS w ON w.id = observed.work_id AND w.state = 'released'
    WHERE refresh.id = refresh_id AND observed.work_id = order_id AND observed.withdrawn_at IS NULL
      AND observed.generation = candidate_generation AND observed.head = candidate_head
      AND observed.verification_sequence = review_sequence;
$$;

CREATE OR REPLACE FUNCTION craft_queue.record_rollout_refresh(
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
    SET verified_at = clock_timestamp(), verified_by = actor_id, rollout_fresh_through = fresh_through
    WHERE refresh.id = refresh_id RETURNING * INTO approved;
    RETURN to_jsonb(approved);
END;
$$;

CREATE OR REPLACE FUNCTION craft_queue.withdraw_series_release(
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
    UPDATE craft_queue.series_observations AS o SET withdrawn_at = clock_timestamp(), withdrawn_by = actor, withdrawal_evidence = evidence_ref
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

CREATE OR REPLACE FUNCTION craft_queue.withdraw_series_on_restore() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog AS $$
BEGIN
    UPDATE craft_queue.series_observations
    SET withdrawn_at = clock_timestamp(), withdrawn_by = NEW.started_by, withdrawal_evidence = 'Restore recovery ' || NEW.epoch::text
    WHERE withdrawn_at IS NULL;
    UPDATE craft_queue.work AS w
    SET state = CASE WHEN craft_queue.team_skill_allowed(w.repository, w.payload)
        AND craft_queue.project_context_allowed(w.repository, w.payload) THEN 'candidate' ELSE 'cancelled' END,
        cancellation_reason = CASE WHEN craft_queue.team_skill_allowed(w.repository, w.payload)
        AND craft_queue.project_context_allowed(w.repository, w.payload) THEN NULL
        ELSE 'Release needs fresh observation after restore and required context or skill was retired' END
    WHERE w.state = 'released' AND EXISTS (SELECT 1 FROM craft_queue.series_members AS s WHERE s.work_id = w.id);
    RETURN NEW;
END;
$$;

CREATE OR REPLACE FUNCTION craft_queue.series_status(plan_id text) RETURNS jsonb
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT jsonb_set(plan.body, '{members}', COALESCE((
        SELECT jsonb_agg(member.value || jsonb_build_object(
            'observer_revision', assignment.observer_revision,
            'latest_observer_transfer', (
                SELECT to_jsonb(event) FROM craft_queue.series_observer_transfers AS event
                WHERE event.work_id = assignment.work_id AND event.revision = assignment.observer_revision
            ),
            'latest_rollout_refresh', (
                SELECT to_jsonb(refresh) FROM craft_queue.series_rollout_refreshes AS refresh
                WHERE refresh.observation_sequence = (member.value #>> '{latest_release,sequence}')::bigint
                  AND refresh.verified_at IS NOT NULL ORDER BY refresh.rollout_fresh_through DESC LIMIT 1
            )
        ) ORDER BY member.ordinality)
        FROM jsonb_array_elements(plan.body->'members') WITH ORDINALITY AS member
        JOIN craft_queue.series_members AS assignment ON assignment.work_id = member.value->>'work_id'
    ), '[]'::jsonb))
    FROM (SELECT craft_queue.series_status_base(plan_id) AS body) AS plan;
$$;

REVOKE ALL ON FUNCTION craft_queue.transfer_release_observer(text,text,text,bigint,text,text) FROM PUBLIC;

COMMIT;
