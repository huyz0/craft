BEGIN;

-- The public API authenticates callers. The service login can execute these
-- transitions without selecting or modifying queue tables directly.
ALTER FUNCTION craft_queue.admit(text, text, text, jsonb, bigint) SECURITY DEFINER;
ALTER FUNCTION craft_queue.claim(text, uuid) SECURITY DEFINER;
ALTER FUNCTION craft_queue.renew(text, text, bigint) SECURITY DEFINER;
ALTER FUNCTION craft_queue.submit_candidate(text, text, bigint, text, text, bigint) SECURITY DEFINER;
ALTER FUNCTION craft_queue.cancel(text, text) SECURITY DEFINER;

CREATE FUNCTION craft_queue.set_paused(pause boolean) RETURNS boolean
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    UPDATE craft_queue.control SET paused = pause WHERE singleton RETURNING paused;
$$;

CREATE FUNCTION craft_queue.status(work_id text) RETURNS jsonb
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT jsonb_build_object(
        'id', w.id, 'repository', w.repository, 'state', w.state,
        'generation', w.generation, 'candidate', w.candidate,
        'candidate_evidence', w.candidate_evidence, 'cancellation_reason', w.cancellation_reason
    )
    FROM craft_queue.work AS w WHERE w.id = work_id;
$$;

REVOKE ALL ON ALL FUNCTIONS IN SCHEMA craft_queue FROM PUBLIC;
COMMIT;
