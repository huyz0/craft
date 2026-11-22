BEGIN;

CREATE FUNCTION craft_queue.verifier_target_repository(order_id text, receipt_id text) RETURNS text
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT w.repository FROM craft_queue.work AS w WHERE w.id = CASE
        WHEN receipt_id IS NULL THEN order_id
        WHEN order_id IS NULL THEN (SELECT r.work_id FROM craft_queue.shadow_merge_receipts AS r WHERE r.id = receipt_id)
    END;
$$;

REVOKE ALL ON FUNCTION craft_queue.verifier_target_repository(text, text) FROM PUBLIC;

COMMIT;
