BEGIN;

CREATE TABLE craft_queue.cutovers (
    sequence bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    epoch uuid NOT NULL UNIQUE,
    phase text NOT NULL CHECK (phase IN ('shared', 'draining', 'released')),
    archive text NOT NULL CHECK (octet_length(archive) <= 524288),
    digest text NOT NULL,
    returned_work jsonb NOT NULL DEFAULT '[]'::jsonb,
    actor text NOT NULL,
    changed_at timestamptz NOT NULL DEFAULT clock_timestamp()
);
CREATE TABLE craft_queue.archived_local_orders (
    work_id text PRIMARY KEY,
    epoch uuid NOT NULL REFERENCES craft_queue.cutovers(epoch)
);

CREATE FUNCTION craft_queue.cutover_status(transfer uuid) RETURNS jsonb
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT jsonb_build_object('epoch', c.epoch, 'phase', c.phase, 'digest', c.digest,
                              'archived_orders', jsonb_array_length((c.archive::jsonb)->'receipts'),
                              'actor', c.actor, 'changed_at', c.changed_at,
                              'returned_work', c.returned_work)
    FROM craft_queue.cutovers AS c WHERE c.epoch = transfer;
$$;

CREATE FUNCTION craft_queue.activate_cutover(transfer uuid, receipt_archive text, operator_id text)
RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    settings craft_queue.control%ROWTYPE;
    existing craft_queue.cutovers%ROWTYPE;
    archive_json jsonb;
BEGIN
    SELECT * INTO STRICT settings FROM craft_queue.control FOR UPDATE;
    SELECT * INTO existing FROM craft_queue.cutovers AS c WHERE c.epoch = transfer;
    IF FOUND THEN
        IF existing.archive <> receipt_archive THEN RAISE EXCEPTION 'cutover archive differs'; END IF;
        RETURN craft_queue.cutover_status(transfer);
    END IF;
    IF NOT settings.paused
        OR EXISTS (SELECT 1 FROM craft_queue.work AS w WHERE w.state <> 'cancelled')
        OR EXISTS (SELECT 1 FROM craft_queue.cutovers AS c WHERE c.phase <> 'released') THEN
        RAISE EXCEPTION 'pause and drain the shared queue before activation';
    END IF;
    IF receipt_archive IS NULL OR octet_length(receipt_archive) > 524288 THEN
        RAISE EXCEPTION 'invalid local receipt archive size';
    END IF;
    archive_json := receipt_archive::jsonb;
    IF archive_json->'schema_version' IS DISTINCT FROM '1'::jsonb
        OR archive_json->>'epoch' IS DISTINCT FROM transfer::text
        OR jsonb_typeof(archive_json->'receipts') IS DISTINCT FROM 'array'
        OR EXISTS (
            SELECT 1 FROM jsonb_array_elements(archive_json->'receipts') AS r(receipt)
            WHERE COALESCE(r.receipt->>'state','') NOT IN ('closed', 'cancelled')
               OR COALESCE(r.receipt->'work_order'->>'id','') !~ '^[A-Za-z0-9_-]{1,100}$'
        ) THEN
        RAISE EXCEPTION 'archive must contain terminal receipts for this epoch';
    END IF;
    INSERT INTO craft_queue.cutovers (epoch, phase, archive, digest, actor)
    VALUES (transfer, 'shared', receipt_archive, 'sha256:' || encode(sha256(convert_to(receipt_archive, 'UTF8')), 'hex'), operator_id);
    INSERT INTO craft_queue.archived_local_orders (work_id, epoch)
    SELECT r.receipt->'work_order'->>'id', transfer
    FROM jsonb_array_elements(archive_json->'receipts') AS r(receipt)
    ON CONFLICT (work_id) DO NOTHING;
    UPDATE craft_queue.control SET paused = false WHERE singleton;
    RETURN craft_queue.cutover_status(transfer);
END;
$$;

CREATE FUNCTION craft_queue.release_cutover(transfer uuid, operator_id text) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    current_phase text;
    newest uuid;
    remaining_work boolean;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    SELECT c.phase INTO current_phase FROM craft_queue.cutovers AS c WHERE c.epoch = transfer;
    IF NOT FOUND THEN RETURN NULL; END IF;
    IF current_phase = 'released' THEN RETURN craft_queue.cutover_status(transfer); END IF;
    SELECT c.epoch INTO newest FROM craft_queue.cutovers AS c ORDER BY c.sequence DESC LIMIT 1;
    IF transfer <> newest THEN RETURN NULL; END IF;

    SELECT EXISTS (SELECT 1 FROM craft_queue.work AS w WHERE w.state <> 'cancelled') INTO remaining_work;
    UPDATE craft_queue.control SET paused = true WHERE singleton;
    UPDATE craft_queue.cutovers AS c
    SET phase = CASE WHEN remaining_work THEN 'draining' ELSE 'released' END,
        returned_work = CASE WHEN remaining_work THEN '[]'::jsonb ELSE (
            SELECT COALESCE(jsonb_agg(w.id ORDER BY w.id), '[]'::jsonb) FROM craft_queue.work AS w
        ) END,
        actor = operator_id, changed_at = clock_timestamp()
    WHERE c.epoch = transfer;
    RETURN craft_queue.cutover_status(transfer);
END;
$$;

CREATE FUNCTION craft_queue.reject_archived_order()
RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog AS $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM craft_queue.archived_local_orders AS a WHERE a.work_id = NEW.id
    ) THEN
        RAISE EXCEPTION 'work order was already archived from the local queue';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER prevent_archived_admission
BEFORE INSERT ON craft_queue.work
FOR EACH ROW EXECUTE FUNCTION craft_queue.reject_archived_order();

CREATE OR REPLACE FUNCTION craft_queue.set_paused(pause boolean) RETURNS boolean
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    latest_phase text;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    SELECT c.phase INTO latest_phase FROM craft_queue.cutovers AS c ORDER BY c.sequence DESC LIMIT 1;
    IF NOT pause AND latest_phase IN ('draining', 'released') THEN
        RAISE EXCEPTION 'shared authority cannot resume after rollback has begun';
    END IF;
    UPDATE craft_queue.control SET paused = pause WHERE singleton;
    RETURN pause;
END;
$$;

CREATE FUNCTION craft_queue.cutover_archive(transfer uuid) RETURNS text
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT c.archive FROM craft_queue.cutovers AS c WHERE c.epoch = transfer;
$$;

REVOKE ALL ON FUNCTION craft_queue.activate_cutover(uuid, text, text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.release_cutover(uuid, text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.cutover_status(uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.cutover_archive(uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.reject_archived_order() FROM PUBLIC;

COMMIT;
