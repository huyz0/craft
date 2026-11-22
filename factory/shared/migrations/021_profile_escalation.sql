BEGIN;

CREATE TABLE craft_queue.profile_escalations (
    source_work_id text PRIMARY KEY REFERENCES craft_queue.work(id),
    replacement_work_id text NOT NULL UNIQUE REFERENCES craft_queue.work(id),
    source_generation bigint NOT NULL,
    actor text NOT NULL,
    evidence text NOT NULL,
    approved_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    FOREIGN KEY (source_work_id, source_generation) REFERENCES craft_queue.attempts(work_id, generation),
    CHECK (source_work_id <> replacement_work_id)
);

CREATE FUNCTION craft_queue.escalate_profile(
    operator_id text, source_id text, expected_generation bigint, replacement_id text,
    profile jsonb, evidence_ref text
) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    source craft_queue.work%ROWTYPE;
    existing craft_queue.profile_escalations%ROWTYPE;
    original_profile jsonb;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    IF NOT craft_queue.execution_profile_valid(profile) OR jsonb_typeof(profile) IS DISTINCT FROM 'object'
        OR length(btrim(COALESCE(evidence_ref, ''))) NOT BETWEEN 1 AND 500 THEN
        RAISE EXCEPTION 'profile escalation requires a valid profile and supporting evidence';
    END IF;
    SELECT * INTO existing FROM craft_queue.profile_escalations AS e WHERE e.source_work_id = source_id;
    IF FOUND THEN
        IF existing.replacement_work_id = replacement_id AND existing.source_generation = expected_generation
            AND existing.actor = operator_id AND existing.evidence = evidence_ref
            AND EXISTS (SELECT 1 FROM craft_queue.work AS w WHERE w.id = replacement_id
                AND w.payload->'execution_profile' = profile) THEN
            RETURN to_jsonb(existing);
        END IF;
        RETURN NULL;
    END IF;

    SELECT * INTO source FROM craft_queue.work AS w WHERE w.id = source_id;
    original_profile := source.payload->'execution_profile';
    IF NOT FOUND OR jsonb_typeof(original_profile) IS DISTINCT FROM 'object'
        OR source.generation IS DISTINCT FROM expected_generation
        OR source.generation < (original_profile->>'max_attempts')::bigint
        OR source.state <> 'leased' OR source.repair_from_generation IS NOT NULL
        OR (original_profile->>'model' = profile->>'model'
            AND original_profile->>'reasoning_effort' = profile->>'reasoning_effort')
        OR NOT EXISTS (SELECT 1 FROM craft_queue.attempts AS a WHERE a.work_id = source_id
            AND a.generation = expected_generation AND a.outcome = 'leased' AND a.expires_at <= clock_timestamp())
        OR EXISTS (SELECT 1 FROM craft_queue.profile_escalations AS e WHERE e.replacement_work_id = source_id)
        OR EXISTS (SELECT 1 FROM craft_queue.standing_work AS s WHERE s.work_id = source_id)
        OR EXISTS (SELECT 1 FROM craft_queue.series_members AS s WHERE s.work_id = source_id)
        OR EXISTS (SELECT 1 FROM craft_queue.publications AS p WHERE p.work_id = source_id)
        OR EXISTS (SELECT 1 FROM craft_queue.work AS w WHERE w.id = replacement_id) THEN
        RETURN NULL;
    END IF;

    PERFORM craft_queue.admit(replacement_id, source.repository, 'profile-escalation:' || source_id,
        source.payload || jsonb_build_object('id', replacement_id, 'execution_profile', profile),
        source.reserved_tokens);
    -- Preserve the original reservation and distinguish expiry from manual cancellation.
    UPDATE craft_queue.attempts SET outcome = 'expired'
    WHERE work_id = source_id AND generation = expected_generation;
    IF NOT craft_queue.cancel(source_id, 'Superseded by approved execution profile ' || replacement_id) THEN
        RAISE EXCEPTION 'could not close the source order';
    END IF;
    INSERT INTO craft_queue.profile_escalations (
        source_work_id, replacement_work_id, source_generation, actor, evidence
    ) VALUES (source_id, replacement_id, expected_generation, operator_id, evidence_ref)
    RETURNING * INTO existing;
    RETURN to_jsonb(existing);
END;
$$;

CREATE FUNCTION craft_queue.profile_escalation_for(work_id text) RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT to_jsonb(e) FROM craft_queue.profile_escalations AS e
    WHERE e.source_work_id = $1 OR e.replacement_work_id = $1;
$$;

REVOKE ALL ON FUNCTION craft_queue.escalate_profile(text, text, bigint, text, jsonb, text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.profile_escalation_for(text) FROM PUBLIC;
COMMIT;
