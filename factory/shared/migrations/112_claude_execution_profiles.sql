BEGIN;

CREATE FUNCTION craft_queue.execution_effort_supported(adapter text, effort text) RETURNS boolean
LANGUAGE sql IMMUTABLE SET search_path = pg_catalog AS $$
    SELECT adapter IS DISTINCT FROM 'claude' OR effort IS DISTINCT FROM 'ultra';
$$;

ALTER TABLE craft_queue.work DROP CONSTRAINT supported_agent_adapter;
ALTER TABLE craft_queue.work ADD CONSTRAINT supported_agent_adapter CHECK (
    NOT (payload ? 'agent_adapter') OR COALESCE(
        payload->'agent_adapter' IN ('"codex"'::jsonb, '"claude"'::jsonb)
        AND craft_queue.execution_effort_supported(payload->>'agent_adapter',
            payload#>>'{execution_profile,reasoning_effort}'),
        false)
);

CREATE FUNCTION craft_queue.guard_fallback_effort() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog AS $$
BEGIN
    IF EXISTS (SELECT 1 FROM craft_queue.work AS source
        WHERE source.id = NEW.source_work_id AND NOT craft_queue.execution_effort_supported(
            source.payload->>'agent_adapter', NEW.execution_profile->>'reasoning_effort')) THEN
        RAISE EXCEPTION 'execution profile effort is unsupported by the source adapter' USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER guard_fallback_effort BEFORE INSERT OR UPDATE OF execution_profile, source_work_id
ON craft_queue.profile_fallbacks FOR EACH ROW EXECUTE FUNCTION craft_queue.guard_fallback_effort();

REVOKE ALL ON FUNCTION craft_queue.execution_effort_supported(text,text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.guard_fallback_effort() FROM PUBLIC;

COMMIT;
