BEGIN;

CREATE FUNCTION craft_queue.recovery_status() RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT to_jsonb(recovery)
    FROM craft_queue.recoveries AS recovery
    ORDER BY (finished_at IS NULL) DESC, started_at DESC, epoch DESC
    LIMIT 1;
$$;

REVOKE ALL ON FUNCTION craft_queue.recovery_status() FROM PUBLIC;

COMMIT;
