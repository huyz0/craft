BEGIN;

CREATE TABLE craft_queue.github_enqueue_policies (
    id text PRIMARY KEY CHECK (id ~ '^[A-Za-z0-9_-]{1,100}$'),
    repository text NOT NULL,
    base_branch text NOT NULL,
    policy jsonb NOT NULL,
    approved_by text NOT NULL,
    approved_at timestamptz NOT NULL,
    expires_at timestamptz NOT NULL,
    revoked_by text,
    revoked_at timestamptz,
    revocation_evidence text
);
ALTER TABLE craft_queue.github_enqueue_intents
    ADD COLUMN standing_policy_id text REFERENCES craft_queue.github_enqueue_policies(id);
CREATE INDEX github_policy_enqueue_window ON craft_queue.github_enqueue_intents(lower(repository), base_branch, approved_at)
    WHERE standing_policy_id IS NOT NULL;

CREATE FUNCTION craft_queue.approve_github_enqueue_policy(actor text, proposed jsonb) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    prior craft_queue.github_enqueue_policies%ROWTYPE;
    now_at timestamptz;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    SELECT p.* INTO prior FROM craft_queue.github_enqueue_policies AS p WHERE p.id = proposed->>'id';
    IF FOUND THEN
        IF prior.approved_by = actor AND prior.policy = proposed AND prior.revoked_at IS NULL
            THEN RETURN to_jsonb(prior); END IF;
        RETURN NULL;
    END IF;
    now_at := clock_timestamp();
    IF EXISTS (SELECT 1 FROM craft_queue.recoveries WHERE finished_at IS NULL)
        OR NOT EXISTS (SELECT 1 FROM craft_queue.shadow_policies AS s
            WHERE s.repository = proposed->>'repository' AND s.digest = proposed->>'shadow_policy_digest'
                AND s.require_native AND s.expires_at > now_at)
        OR EXISTS (SELECT 1 FROM craft_queue.github_enqueue_policies AS p
            WHERE lower(p.repository) = lower(proposed->>'repository') AND p.base_branch = proposed->>'base_branch'
                AND p.revoked_at IS NULL AND p.expires_at > now_at)
        THEN RETURN NULL; END IF;
    INSERT INTO craft_queue.github_enqueue_policies(id, repository, base_branch, policy, approved_by, approved_at, expires_at)
    VALUES (proposed->>'id', proposed->>'repository', proposed->>'base_branch', proposed, actor,
        now_at, now_at + make_interval(secs => (proposed->>'valid_seconds')::integer))
    RETURNING * INTO prior;
    RETURN to_jsonb(prior);
END;
$$;

CREATE FUNCTION craft_queue.revoke_github_enqueue_policy(actor text, policy_id text, evidence_ref text) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    prior craft_queue.github_enqueue_policies%ROWTYPE;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    SELECT p.* INTO prior FROM craft_queue.github_enqueue_policies AS p WHERE p.id = policy_id;
    IF NOT FOUND THEN RETURN NULL; END IF;
    IF prior.revoked_at IS NOT NULL THEN
        IF prior.revoked_by = actor AND prior.revocation_evidence = evidence_ref THEN RETURN to_jsonb(prior); END IF;
        RETURN NULL;
    END IF;
    UPDATE craft_queue.github_enqueue_policies AS p SET revoked_at = clock_timestamp(), revoked_by = actor,
        revocation_evidence = evidence_ref WHERE p.id = policy_id RETURNING * INTO prior;
    RETURN to_jsonb(prior);
END;
$$;

CREATE FUNCTION craft_queue.authorize_github_enqueue_policy(
    publisher_id text, policy_id text, intent text, receipt text, expected_head text, report_pin text
) RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    rule craft_queue.github_enqueue_policies%ROWTYPE;
    prior craft_queue.github_enqueue_intents%ROWTYPE;
    target jsonb;
    risk jsonb;
    native_pin text;
    approved jsonb;
    now_at timestamptz;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    now_at := clock_timestamp();
    SELECT p.* INTO rule FROM craft_queue.github_enqueue_policies AS p
    WHERE p.id = policy_id AND p.revoked_at IS NULL AND p.expires_at > now_at
        AND p.policy->>'publisher' = publisher_id;
    IF NOT FOUND OR EXISTS (SELECT 1 FROM craft_queue.control WHERE paused) THEN RETURN NULL; END IF;
    SELECT i.* INTO prior FROM craft_queue.github_enqueue_intents AS i WHERE i.id = intent;
    IF FOUND THEN
        IF prior.standing_policy_id = policy_id AND prior.receipt_id = receipt AND prior.head = expected_head
            AND EXISTS (SELECT 1 FROM craft_queue.shadow_merge_receipts AS r
                WHERE r.id = receipt AND r.report_digest = report_pin)
            THEN RETURN to_jsonb(prior); END IF;
        RETURN NULL;
    END IF;
    target := craft_queue.current_github_enqueue(receipt, expected_head, report_pin);
    IF target IS NULL OR target->>'repository' IS DISTINCT FROM rule.repository
        OR target->>'base_branch' IS DISTINCT FROM rule.base_branch THEN RETURN NULL; END IF;
    SELECT r.assessment->'enqueue_risk', r.policy_digest INTO risk, native_pin
    FROM craft_queue.shadow_merge_receipts AS r WHERE r.id = receipt;
    IF native_pin IS DISTINCT FROM rule.policy->>'shadow_policy_digest'
        OR jsonb_typeof(risk->'paths') IS DISTINCT FROM 'array'
        OR (risk->>'commits')::integer NOT BETWEEN 1 AND (rule.policy->>'max_commits')::integer
        OR (risk->>'changed_lines')::integer NOT BETWEEN 1 AND (rule.policy->>'max_changed_lines')::integer
        OR risk->>'commits' IS NULL OR risk->>'changed_lines' IS NULL
        THEN RETURN NULL; END IF;
    IF jsonb_array_length(risk->'paths') NOT BETWEEN 1 AND (rule.policy->>'max_files')::integer
        OR NOT ((rule.policy->'allowed_paths') @> (risk->'paths'))
        OR (SELECT count(*) FROM craft_queue.github_enqueue_intents AS i
            WHERE lower(i.repository) = lower(rule.repository) AND i.base_branch = rule.base_branch
                AND i.standing_policy_id IS NOT NULL AND (i.state <> 'abandoned' OR i.send_possible)
                AND i.approved_at > now_at - interval '24 hours') >= (rule.policy->>'daily_limit')::integer
        THEN RETURN NULL; END IF;
    approved := craft_queue.approve_github_enqueue(rule.approved_by, intent, receipt, expected_head, report_pin,
        publisher_id, rule.policy->>'evidence', 300);
    IF approved IS NULL THEN RETURN NULL; END IF;
    UPDATE craft_queue.github_enqueue_intents AS i SET standing_policy_id = policy_id
    WHERE i.id = intent RETURNING * INTO prior;
    RETURN to_jsonb(prior);
END;
$$;

ALTER FUNCTION craft_queue.prepare_github_enqueue(text,text,boolean) RENAME TO prepare_github_enqueue_before_policy;
CREATE FUNCTION craft_queue.prepare_github_enqueue(actor text, intent text, start_send boolean) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    item craft_queue.github_enqueue_intents%ROWTYPE;
    binding jsonb;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    SELECT i.* INTO item FROM craft_queue.github_enqueue_intents AS i WHERE i.id = intent AND i.publisher = actor;
    IF NOT FOUND THEN RETURN NULL; END IF;
    IF item.standing_policy_id IS NOT NULL AND NOT EXISTS (
        SELECT 1 FROM craft_queue.github_enqueue_policies AS p
        WHERE p.id = item.standing_policy_id AND p.revoked_at IS NULL AND p.expires_at > clock_timestamp()
    ) THEN RETURN NULL; END IF;
    binding := craft_queue.prepare_github_enqueue_before_policy(actor, intent, start_send);
    IF binding IS NOT NULL AND NOT start_send AND item.standing_policy_id IS NOT NULL THEN
        SELECT jsonb_set(binding, '{target,risk}', r.assessment->'enqueue_risk') INTO binding
        FROM craft_queue.shadow_merge_receipts AS r WHERE r.id = item.receipt_id;
    END IF;
    RETURN binding;
END;
$$;

CREATE FUNCTION craft_queue.revoke_github_enqueue_policies_after_restore() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog AS $$
BEGIN
    UPDATE craft_queue.github_enqueue_policies SET revoked_at = NEW.started_at,
        revoked_by = NEW.started_by, revocation_evidence = 'Restore recovery ' || NEW.epoch::text
    WHERE revoked_at IS NULL;
    RETURN NEW;
END;
$$;
CREATE TRIGGER revoke_github_enqueue_policies_after_restore AFTER INSERT ON craft_queue.recoveries
    FOR EACH ROW EXECUTE FUNCTION craft_queue.revoke_github_enqueue_policies_after_restore();

REVOKE ALL ON FUNCTION craft_queue.approve_github_enqueue_policy(text,jsonb) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.revoke_github_enqueue_policy(text,text,text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.authorize_github_enqueue_policy(text,text,text,text,text,text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.prepare_github_enqueue(text,text,boolean) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.revoke_github_enqueue_policies_after_restore() FROM PUBLIC;

COMMIT;
