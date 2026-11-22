BEGIN;

CREATE TABLE craft_queue.profile_fallbacks (
    source_work_id text PRIMARY KEY REFERENCES craft_queue.work(id),
    source_generation bigint NOT NULL CHECK (source_generation BETWEEN 1 AND 12),
    replacement_work_id text NOT NULL UNIQUE CHECK (replacement_work_id ~ '^[A-Za-z0-9_-]{1,100}$'),
    execution_profile jsonb NOT NULL CHECK (
        jsonb_typeof(execution_profile) = 'object' AND craft_queue.execution_profile_valid(execution_profile)
    ),
    approved_by text NOT NULL,
    evidence text NOT NULL,
    approved_at timestamptz NOT NULL,
    expires_at timestamptz NOT NULL CHECK (expires_at > approved_at),
    revoked_at timestamptz,
    revoked_by text,
    applied_at timestamptz,
    applied_by text,
    blocked_at timestamptz
);
CREATE INDEX profile_fallbacks_pending ON craft_queue.profile_fallbacks (expires_at)
WHERE revoked_at IS NULL AND applied_at IS NULL AND blocked_at IS NULL;

CREATE FUNCTION craft_queue.approve_profile_fallback(
    actor text, source_id text, replacement_id text, profile jsonb, decision_ref text, valid_seconds integer
) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    source craft_queue.work%ROWTYPE;
    existing craft_queue.profile_fallbacks%ROWTYPE;
    original_profile jsonb;
    approved timestamptz;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    IF valid_seconds IS NULL OR valid_seconds NOT BETWEEN 1 AND 604800
       OR jsonb_typeof(profile) IS DISTINCT FROM 'object' OR NOT craft_queue.execution_profile_valid(profile)
       OR length(btrim(COALESCE(decision_ref, ''))) NOT BETWEEN 1 AND 500 THEN
        RETURN NULL;
    END IF;
    SELECT * INTO existing FROM craft_queue.profile_fallbacks AS f WHERE f.source_work_id = source_id;
    IF FOUND THEN
        IF existing.replacement_work_id = replacement_id AND existing.execution_profile = profile
            AND existing.approved_by = actor AND existing.evidence = decision_ref
            AND existing.expires_at - existing.approved_at = make_interval(secs => valid_seconds) THEN
            RETURN to_jsonb(existing);
        END IF;
        RETURN NULL;
    END IF;
    SELECT * INTO source FROM craft_queue.work AS w WHERE w.id = source_id;
    original_profile := source.payload->'execution_profile';
    IF NOT FOUND OR source.state NOT IN ('queued', 'leased') OR source.repair_from_generation IS NOT NULL
        OR jsonb_typeof(original_profile) IS DISTINCT FROM 'object'
        OR (original_profile->>'model' = profile->>'model'
            AND original_profile->>'reasoning_effort' = profile->>'reasoning_effort')
        OR EXISTS (SELECT 1 FROM craft_queue.work AS w WHERE w.id = replacement_id)
        OR EXISTS (SELECT 1 FROM craft_queue.profile_escalations AS e
            WHERE e.source_work_id = source_id OR e.replacement_work_id = source_id)
        OR EXISTS (SELECT 1 FROM craft_queue.standing_work AS s WHERE s.work_id = source_id)
        OR EXISTS (SELECT 1 FROM craft_queue.series_members AS s WHERE s.work_id = source_id)
        OR EXISTS (SELECT 1 FROM craft_queue.publications AS p WHERE p.work_id = source_id)
        OR EXISTS (SELECT 1 FROM craft_queue.recoveries WHERE finished_at IS NULL) THEN
        RETURN NULL;
    END IF;
    approved := clock_timestamp();
    INSERT INTO craft_queue.profile_fallbacks (
        source_work_id, source_generation, replacement_work_id, execution_profile,
        approved_by, evidence, approved_at, expires_at
    ) VALUES (source_id, (original_profile->>'max_attempts')::bigint, replacement_id, profile,
        actor, decision_ref, approved, approved + make_interval(secs => valid_seconds))
    ON CONFLICT DO NOTHING RETURNING * INTO existing;
    IF NOT FOUND THEN RETURN NULL; END IF;
    RETURN to_jsonb(existing);
END;
$$;

CREATE FUNCTION craft_queue.revoke_profile_fallback(actor text, source_id text) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    approval craft_queue.profile_fallbacks%ROWTYPE;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    UPDATE craft_queue.profile_fallbacks AS f SET revoked_at = clock_timestamp(), revoked_by = actor
    WHERE f.source_work_id = source_id AND f.applied_at IS NULL AND f.revoked_at IS NULL
    RETURNING * INTO approval;
    IF FOUND THEN RETURN to_jsonb(approval); END IF;
    SELECT * INTO approval FROM craft_queue.profile_fallbacks AS f
    WHERE f.source_work_id = source_id AND f.revoked_by = actor;
    IF NOT FOUND THEN RETURN NULL; END IF;
    RETURN to_jsonb(approval);
END;
$$;

CREATE FUNCTION craft_queue.apply_profile_fallbacks(scheduler text) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    paused boolean;
    approval craft_queue.profile_fallbacks%ROWTYPE;
    escalation jsonb;
    applied jsonb := '[]'::jsonb;
    blocked jsonb := '[]'::jsonb;
    now_at timestamptz;
BEGIN
    SELECT c.paused INTO STRICT paused FROM craft_queue.control AS c FOR UPDATE;
    IF paused THEN RETURN jsonb_build_object('paused', true, 'applied', applied, 'blocked', blocked); END IF;
    now_at := clock_timestamp();
    FOR approval IN
        SELECT f.* FROM craft_queue.profile_fallbacks AS f
        JOIN craft_queue.work AS w ON w.id = f.source_work_id
        JOIN craft_queue.attempts AS a ON a.work_id = w.id AND a.generation = w.generation
        WHERE f.revoked_at IS NULL AND f.applied_at IS NULL AND f.blocked_at IS NULL
          AND f.expires_at > now_at AND w.state = 'leased' AND w.generation = f.source_generation
          AND a.outcome = 'leased' AND a.expires_at <= now_at
        ORDER BY f.expires_at, f.source_work_id LIMIT 20
    LOOP
        IF approval.expires_at <= clock_timestamp() THEN CONTINUE; END IF;
        BEGIN
            escalation := craft_queue.escalate_profile(approval.approved_by, approval.source_work_id,
                approval.source_generation, approval.replacement_work_id, approval.execution_profile, approval.evidence);
        EXCEPTION WHEN raise_exception OR unique_violation OR check_violation THEN
            escalation := NULL;
        END;
        IF escalation IS NULL THEN
            UPDATE craft_queue.profile_fallbacks AS f SET blocked_at = clock_timestamp()
            WHERE f.source_work_id = approval.source_work_id;
            blocked := blocked || jsonb_build_array(approval.source_work_id);
        ELSE
            UPDATE craft_queue.profile_fallbacks AS f SET applied_at = clock_timestamp(), applied_by = scheduler
            WHERE f.source_work_id = approval.source_work_id;
            applied := applied || jsonb_build_array(escalation);
        END IF;
    END LOOP;
    RETURN jsonb_build_object('paused', false, 'applied', applied, 'blocked', blocked);
END;
$$;

CREATE FUNCTION craft_queue.profile_fallback_for(work_id text) RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT to_jsonb(f) FROM craft_queue.profile_fallbacks AS f WHERE f.source_work_id = $1;
$$;

CREATE FUNCTION craft_queue.revoke_profile_fallbacks_on_restore() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
BEGIN
    UPDATE craft_queue.profile_fallbacks SET revoked_at = NEW.started_at, revoked_by = 'restore:' || NEW.epoch::text
    WHERE applied_at IS NULL AND revoked_at IS NULL;
    RETURN NEW;
END;
$$;
CREATE TRIGGER revoke_profile_fallbacks_on_restore AFTER INSERT ON craft_queue.recoveries
FOR EACH ROW EXECUTE FUNCTION craft_queue.revoke_profile_fallbacks_on_restore();

REVOKE ALL ON FUNCTION craft_queue.approve_profile_fallback(text, text, text, jsonb, text, integer) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.revoke_profile_fallback(text, text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.apply_profile_fallbacks(text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.profile_fallback_for(text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.revoke_profile_fallbacks_on_restore() FROM PUBLIC;
COMMIT;
