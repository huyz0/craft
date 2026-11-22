BEGIN;

CREATE TABLE craft_queue.github_enqueue_intents (
    id text PRIMARY KEY CHECK (id ~ '^[A-Za-z0-9_-]{1,100}$'),
    receipt_id text NOT NULL REFERENCES craft_queue.shadow_merge_receipts(id),
    work_id text NOT NULL REFERENCES craft_queue.work(id),
    repository text NOT NULL,
    pull_request text NOT NULL,
    head text NOT NULL,
    base_branch text NOT NULL,
    pull_request_node_id text NOT NULL,
    publisher text NOT NULL,
    approved_by text NOT NULL,
    approval text NOT NULL,
    approved_at timestamptz NOT NULL,
    expires_at timestamptz NOT NULL,
    state text NOT NULL CHECK (state IN ('approved', 'sending', 'uncertain', 'queued', 'abandoned')),
    send_possible boolean NOT NULL DEFAULT false,
    sending_at timestamptz,
    resolution_actor text,
    resolution_evidence text,
    resolution_at timestamptz,
    queue_entry_id text
);
CREATE UNIQUE INDEX github_enqueue_once_per_head
    ON craft_queue.github_enqueue_intents(lower(repository), pull_request, head)
    WHERE state <> 'abandoned' OR send_possible;
CREATE INDEX github_enqueue_by_work ON craft_queue.github_enqueue_intents(work_id, approved_at DESC);

-- The readiness report is approved evidence. The sender separately repeats native inspection.
CREATE FUNCTION craft_queue.current_github_enqueue(receipt text, expected_head text, report_pin text)
RETURNS jsonb LANGUAGE sql STABLE SET search_path = pg_catalog AS $$
    SELECT jsonb_build_object('work_id', r.work_id, 'repository', r.repository, 'head', r.head,
        'pull_request', p.result_url, 'base', p.base, 'base_branch', p.base_branch,
        'readiness', r.assessment->'github_queue', 'github_policy', r.assessment->'github',
        'work_order', w.payload)
    FROM craft_queue.shadow_merge_receipts AS r
    JOIN craft_queue.work AS w ON w.id = r.work_id
    JOIN craft_queue.shadow_policies AS policy ON policy.repository = r.repository AND policy.digest = r.policy_digest
    JOIN craft_queue.publications AS p ON p.action_key = r.publication_key
    WHERE r.id = receipt AND r.head = expected_head AND r.report_digest = report_pin
      AND p.repository = r.repository AND p.result_url = r.assessment->>'pull_request'
      AND p.result_url = 'https://github.com/' || r.repository || '/pull/' || split_part(p.result_url, '/pull/', 2)
      AND split_part(p.result_url, '/pull/', 2) ~ '^[1-9][0-9]{0,9}$'
      AND r.assessment->'eligible' = 'true'::jsonb AND r.assessment->'native_verified' = 'true'::jsonb
      AND r.assessment->'github_queue'->>'pull_request_node_id' IS NOT NULL
      AND policy.require_native AND policy.expires_at > statement_timestamp()
      AND r.recorded_at > statement_timestamp() - interval '10 minutes'
      AND (r.assessment->>'as_of')::bigint >= extract(epoch FROM statement_timestamp())::bigint - 600
      AND p.action_key = craft_queue.current_review_draft(r.work_id, r.generation, r.head, r.verification_sequence)
      AND NOT EXISTS (SELECT 1 FROM craft_queue.recoveries AS recovery WHERE recovery.finished_at IS NULL)
      AND NOT EXISTS (SELECT 1 FROM craft_queue.publications AS other
          WHERE other.work_id = w.id AND other.state IN ('reserved', 'sending', 'uncertain'));
$$;

CREATE FUNCTION craft_queue.approve_github_enqueue(
    actor text, intent text, receipt text, expected_head text, report_pin text,
    assigned_publisher text, evidence_ref text, lifetime integer
) RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    prior craft_queue.github_enqueue_intents%ROWTYPE;
    target jsonb;
    approved_time timestamptz;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    SELECT i.* INTO prior FROM craft_queue.github_enqueue_intents AS i WHERE i.id = intent;
    IF FOUND THEN
        IF prior.approved_by = actor AND prior.publisher = assigned_publisher
            AND prior.receipt_id = receipt AND prior.head = expected_head AND prior.approval = evidence_ref
            AND prior.expires_at = prior.approved_at + make_interval(secs => lifetime)
            AND EXISTS (SELECT 1 FROM craft_queue.shadow_merge_receipts AS r
                WHERE r.id = receipt AND r.report_digest = report_pin)
        THEN RETURN to_jsonb(prior); END IF;
        RETURN NULL;
    END IF;
    target := craft_queue.current_github_enqueue(receipt, expected_head, report_pin);
    IF target IS NULL OR assigned_publisher = actor OR lifetime NOT BETWEEN 1 AND 600
        OR length(btrim(evidence_ref)) NOT BETWEEN 1 AND 500 THEN RETURN NULL; END IF;
    approved_time := clock_timestamp();
    INSERT INTO craft_queue.github_enqueue_intents(id, receipt_id, work_id, repository, pull_request, head,
        base_branch, pull_request_node_id, publisher, approved_by, approval, approved_at, expires_at, state)
    VALUES (intent, receipt, target->>'work_id', target->>'repository', target->>'pull_request', expected_head,
        target->>'base_branch', target#>>'{readiness,pull_request_node_id}', assigned_publisher,
        actor, evidence_ref, approved_time, approved_time + make_interval(secs => lifetime), 'approved')
    ON CONFLICT DO NOTHING RETURNING * INTO prior;
    IF NOT FOUND THEN RETURN NULL; END IF;
    RETURN to_jsonb(prior);
END;
$$;

CREATE FUNCTION craft_queue.github_enqueue_status(intent text) RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT to_jsonb(i) FROM craft_queue.github_enqueue_intents AS i WHERE i.id = intent;
$$;

CREATE FUNCTION craft_queue.github_enqueue_holds(after_intent text) RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT COALESCE(jsonb_agg(to_jsonb(page) ORDER BY page.id), '[]'::jsonb)
    FROM (SELECT i.* FROM craft_queue.github_enqueue_intents AS i
        WHERE i.state IN ('sending', 'uncertain') AND (after_intent IS NULL OR i.id > after_intent)
        ORDER BY i.id LIMIT 50) AS page;
$$;

CREATE FUNCTION craft_queue.prepare_github_enqueue(actor text, intent text, start_send boolean) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    item craft_queue.github_enqueue_intents%ROWTYPE;
    target jsonb;
    report_pin text;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    SELECT i.* INTO item FROM craft_queue.github_enqueue_intents AS i
    WHERE i.id = intent AND i.publisher = actor AND i.state = 'approved';
    IF NOT FOUND OR item.expires_at <= clock_timestamp()
        OR EXISTS (SELECT 1 FROM craft_queue.control WHERE paused) THEN RETURN NULL; END IF;
    SELECT r.report_digest INTO report_pin FROM craft_queue.shadow_merge_receipts AS r WHERE r.id = item.receipt_id;
    target := craft_queue.current_github_enqueue(item.receipt_id, item.head, report_pin);
    IF target IS NULL THEN RETURN NULL; END IF;
    IF start_send THEN
        UPDATE craft_queue.github_enqueue_intents AS i SET state = 'sending',
            send_possible = true, sending_at = clock_timestamp()
        WHERE i.id = intent RETURNING * INTO item;
        RETURN jsonb_build_object('intent', to_jsonb(item));
    END IF;
    RETURN jsonb_build_object('intent', to_jsonb(item), 'target', target);
END;
$$;

CREATE FUNCTION craft_queue.resolve_github_enqueue(
    actor text, intent text, operator_actor boolean, outcome text, entry text, evidence_ref text
) RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    item craft_queue.github_enqueue_intents%ROWTYPE;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    SELECT i.* INTO item FROM craft_queue.github_enqueue_intents AS i WHERE i.id = intent;
    IF NOT FOUND OR (NOT operator_actor AND actor <> item.publisher) THEN RETURN NULL; END IF;
    IF item.state = outcome AND item.resolution_actor = actor AND item.resolution_evidence = evidence_ref
        AND item.queue_entry_id IS NOT DISTINCT FROM entry THEN RETURN to_jsonb(item); END IF;
    IF length(btrim(evidence_ref)) NOT BETWEEN 1 AND 500 OR (outcome = 'queued') <> (entry IS NOT NULL)
        OR (operator_actor AND NOT (
            (outcome = 'queued' AND item.state IN ('sending', 'uncertain')) OR
            (outcome = 'abandoned' AND item.state IN ('approved', 'sending', 'uncertain'))))
        OR (NOT operator_actor AND (item.state <> 'sending' OR outcome NOT IN ('queued', 'uncertain')))
        THEN RETURN NULL; END IF;
    UPDATE craft_queue.github_enqueue_intents AS i
    SET state = outcome, queue_entry_id = entry, resolution_actor = actor,
        resolution_evidence = evidence_ref, resolution_at = clock_timestamp()
    WHERE i.id = intent RETURNING * INTO item;
    RETURN to_jsonb(item);
END;
$$;

CREATE FUNCTION craft_queue.hold_github_enqueue_after_restore() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog AS $$
BEGIN
    UPDATE craft_queue.github_enqueue_intents AS i SET state = 'uncertain', send_possible = true,
        resolution_actor = NEW.started_by, resolution_at = NEW.started_at,
        resolution_evidence = 'Restore recovery ' || NEW.epoch::text
    WHERE i.state IN ('approved', 'sending', 'uncertain');
    RETURN NEW;
END;
$$;
CREATE TRIGGER hold_github_enqueue_after_restore AFTER INSERT ON craft_queue.recoveries
    FOR EACH ROW EXECUTE FUNCTION craft_queue.hold_github_enqueue_after_restore();

ALTER FUNCTION craft_queue.finish_recovery(uuid, text, text) RENAME TO finish_recovery_before_github_enqueue;
CREATE FUNCTION craft_queue.finish_recovery(recovery_id uuid, actor text, evidence_ref text) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    IF EXISTS (SELECT 1 FROM craft_queue.recoveries AS r WHERE r.epoch = recovery_id AND r.finished_at IS NULL)
        AND EXISTS (SELECT 1 FROM craft_queue.github_enqueue_intents AS i WHERE i.state IN ('sending', 'uncertain'))
        THEN RETURN NULL; END IF;
    RETURN craft_queue.finish_recovery_before_github_enqueue(recovery_id, actor, evidence_ref);
END;
$$;

CREATE FUNCTION craft_queue.guard_github_enqueue_work() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog AS $$
BEGIN
    IF ROW(NEW.state, NEW.generation, NEW.candidate) IS DISTINCT FROM ROW(OLD.state, OLD.generation, OLD.candidate)
        AND EXISTS (SELECT 1 FROM craft_queue.github_enqueue_intents AS i
            WHERE i.work_id = OLD.id AND i.state IN ('sending', 'uncertain'))
        THEN RAISE EXCEPTION 'reconcile the GitHub enqueue before changing the work state'; END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER guard_github_enqueue_work BEFORE UPDATE OF state, generation, candidate ON craft_queue.work
    FOR EACH ROW EXECUTE FUNCTION craft_queue.guard_github_enqueue_work();

CREATE FUNCTION craft_queue.github_enqueue_intents_for(order_id text) RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT COALESCE(jsonb_agg(to_jsonb(recent) ORDER BY recent.approved_at DESC, recent.id), '[]'::jsonb)
    FROM (SELECT i.* FROM craft_queue.github_enqueue_intents AS i WHERE i.work_id = order_id
        ORDER BY i.approved_at DESC, i.id LIMIT 20) AS recent;
$$;

REVOKE ALL ON FUNCTION craft_queue.current_github_enqueue(text,text,text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.approve_github_enqueue(text,text,text,text,text,text,text,integer) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.github_enqueue_status(text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.github_enqueue_holds(text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.prepare_github_enqueue(text,text,boolean) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.resolve_github_enqueue(text,text,boolean,text,text,text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.hold_github_enqueue_after_restore() FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.finish_recovery(uuid,text,text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.guard_github_enqueue_work() FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.github_enqueue_intents_for(text) FROM PUBLIC;

COMMIT;
