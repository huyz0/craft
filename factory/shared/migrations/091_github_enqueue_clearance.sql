BEGIN;

ALTER TABLE craft_queue.github_enqueue_intents
    DROP CONSTRAINT github_enqueue_intents_state_check,
    ADD CHECK (state IN ('approved', 'sending', 'uncertain', 'queued', 'abandoned', 'cleared', 'merged')),
    ADD COLUMN cleared_at timestamptz,
    ADD COLUMN cleared_by text,
    ADD COLUMN clear_evidence text,
    ADD COLUMN clear_observed_head text,
    ADD COLUMN merge_observation_receipt_id text REFERENCES craft_queue.github_shadow_merges(receipt_id);
CREATE INDEX github_enqueue_active_work ON craft_queue.github_enqueue_intents(work_id)
    WHERE state IN ('sending', 'uncertain', 'queued');

CREATE FUNCTION craft_queue.github_enqueue_blocks_work(order_id text) RETURNS boolean
LANGUAGE sql STABLE SET search_path = pg_catalog AS $$
    SELECT EXISTS (SELECT 1 FROM craft_queue.github_enqueue_intents AS i
        WHERE i.work_id = order_id AND i.state IN ('sending', 'uncertain', 'queued'));
$$;

CREATE OR REPLACE FUNCTION craft_queue.guard_github_enqueue_work() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog AS $$
BEGIN
    -- Restore must be able to fence old leases before serving requests to reconcile holds.
    IF OLD.state = 'leased' AND NEW.state = 'queued'
        AND ROW(NEW.generation, NEW.candidate) IS NOT DISTINCT FROM ROW(OLD.generation, OLD.candidate)
        AND EXISTS (SELECT 1 FROM craft_queue.recoveries AS r WHERE r.finished_at IS NULL)
        THEN RETURN NEW; END IF;
    IF ROW(NEW.state, NEW.generation, NEW.candidate) IS DISTINCT FROM ROW(OLD.state, OLD.generation, OLD.candidate)
        AND craft_queue.github_enqueue_blocks_work(OLD.id)
        THEN RAISE EXCEPTION 'clear the GitHub queue entry or confirm the merge before changing the work state'; END IF;
    RETURN NEW;
END;
$$;

-- Automatic repair may be blocked without rolling back the human review that requested it.
ALTER FUNCTION craft_queue.request_repair(text,text,bigint,text,text,text,text)
    RENAME TO request_repair_before_github_enqueue_clearance;
CREATE FUNCTION craft_queue.request_repair(
    actor text, order_id text, candidate_generation bigint, candidate_head text,
    draft_url text, evidence_ref text, feedback text
) RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    IF craft_queue.github_enqueue_blocks_work(order_id) THEN RETURN NULL; END IF;
    RETURN craft_queue.request_repair_before_github_enqueue_clearance(
        actor, order_id, candidate_generation, candidate_head, draft_url, evidence_ref, feedback);
END;
$$;

CREATE FUNCTION craft_queue.clear_github_enqueue(
    actor text, intent text, observed_node text, observed_head text, evidence_ref text
) RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    item craft_queue.github_enqueue_intents%ROWTYPE;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    SELECT i.* INTO item FROM craft_queue.github_enqueue_intents AS i WHERE i.id = intent;
    IF NOT FOUND OR item.pull_request_node_id IS DISTINCT FROM observed_node
        OR COALESCE(observed_head, '') !~ '^([a-f0-9]{40}|[a-f0-9]{64})$'
        OR length(btrim(COALESCE(evidence_ref, ''))) NOT BETWEEN 1 AND 500 THEN RETURN NULL; END IF;
    IF item.state = 'cleared' THEN
        IF item.cleared_by = actor AND item.clear_evidence = evidence_ref AND item.clear_observed_head = observed_head
            THEN RETURN to_jsonb(item); END IF;
        RETURN NULL;
    END IF;
    IF item.state <> 'queued' THEN RETURN NULL; END IF;
    UPDATE craft_queue.github_enqueue_intents AS i SET state = 'cleared', cleared_by = actor,
        cleared_at = clock_timestamp(), clear_observed_head = observed_head, clear_evidence = evidence_ref
    WHERE i.id = intent RETURNING * INTO item;
    RETURN to_jsonb(item);
END;
$$;

CREATE FUNCTION craft_queue.github_enqueue_waits(after_intent text, include_queued boolean) RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT COALESCE(jsonb_agg(to_jsonb(page) ORDER BY page.id), '[]'::jsonb)
    FROM (SELECT i.* FROM craft_queue.github_enqueue_intents AS i
        WHERE (i.state IN ('sending', 'uncertain') OR (include_queued AND i.state = 'queued'))
          AND (after_intent IS NULL OR i.id > after_intent)
        ORDER BY i.id LIMIT 50) AS page;
$$;

CREATE FUNCTION craft_queue.mark_github_enqueue_merged() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog AS $$
BEGIN
    UPDATE craft_queue.github_enqueue_intents AS i
    SET state = 'merged', merge_observation_receipt_id = NEW.receipt_id
    FROM craft_queue.shadow_merge_receipts AS source, craft_queue.shadow_merge_receipts AS observed
    WHERE source.id = i.receipt_id AND observed.id = NEW.receipt_id
      AND source.work_id = observed.work_id AND source.generation = observed.generation
      AND source.head = observed.head AND source.publication_key = observed.publication_key
      AND NEW.observation->>'head' = i.head AND NEW.observation->>'pull_request' = i.pull_request
      AND i.state IN ('sending', 'uncertain', 'queued');
    RETURN NEW;
END;
$$;
CREATE TRIGGER mark_github_enqueue_merged AFTER INSERT ON craft_queue.github_shadow_merges
    FOR EACH ROW EXECUTE FUNCTION craft_queue.mark_github_enqueue_merged();

-- A later release may already have been confirmed after migration 090.
WITH confirmed AS (
    SELECT DISTINCT ON (i.id) i.id, m.receipt_id
    FROM craft_queue.github_enqueue_intents AS i
    JOIN craft_queue.shadow_merge_receipts AS source ON source.id = i.receipt_id
    JOIN craft_queue.shadow_merge_receipts AS observed ON observed.work_id = source.work_id
        AND observed.generation = source.generation AND observed.head = source.head
        AND observed.publication_key = source.publication_key
    JOIN craft_queue.github_shadow_merges AS m ON m.receipt_id = observed.id
    WHERE i.state IN ('sending', 'uncertain', 'queued') AND m.observation->>'head' = i.head
        AND m.observation->>'pull_request' = i.pull_request
    ORDER BY i.id, m.recorded_at, m.receipt_id
)
UPDATE craft_queue.github_enqueue_intents AS i
SET state = 'merged', merge_observation_receipt_id = confirmed.receipt_id
FROM confirmed WHERE i.id = confirmed.id;

-- Craft review decisions also fence send authorization, even before the forge reports them.
ALTER FUNCTION craft_queue.current_github_enqueue(text,text,text)
    RENAME TO current_github_enqueue_before_clearance;
CREATE FUNCTION craft_queue.current_github_enqueue(receipt text, expected_head text, report_pin text)
RETURNS jsonb LANGUAGE sql STABLE SET search_path = pg_catalog AS $$
    SELECT craft_queue.current_github_enqueue_before_clearance(receipt, expected_head, report_pin)
    WHERE NOT EXISTS (
        SELECT 1 FROM craft_queue.shadow_merge_receipts AS r
        JOIN craft_queue.review_handoffs AS h ON h.work_id = r.work_id AND h.generation = r.generation
            AND h.head = r.head AND h.publication_key = r.publication_key
        WHERE r.id = receipt AND h.decision = 'changes_requested'
    );
$$;

REVOKE ALL ON FUNCTION craft_queue.github_enqueue_blocks_work(text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.request_repair(text,text,bigint,text,text,text,text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.clear_github_enqueue(text,text,text,text,text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.github_enqueue_waits(text,boolean) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.mark_github_enqueue_merged() FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.current_github_enqueue(text,text,text) FROM PUBLIC;

COMMIT;
