BEGIN;

CREATE FUNCTION craft_queue.matches_migration_history(expected text[]) RETURNS boolean
LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
BEGIN
    IF to_regclass('craft_migration.queue_history') IS NULL THEN
        RETURN false;
    END IF;
    RETURN (SELECT array_agg(number::text || ' ' || filename || ' ' || digest ORDER BY number)
            FROM craft_migration.queue_history) IS NOT DISTINCT FROM expected;
END;
$$;

REVOKE ALL ON FUNCTION craft_queue.matches_migration_history(text[]) FROM PUBLIC;

COMMIT;
