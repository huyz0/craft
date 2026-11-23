BEGIN;

ALTER TABLE craft_queue.diagnostic_grants
    ADD COLUMN grant_id text NOT NULL DEFAULT gen_random_uuid()::text
        CHECK (grant_id ~ '^[A-Za-z0-9_-]{1,100}$'),
    ADD COLUMN issued_by text,
    ADD COLUMN issued_at timestamptz,
    ADD COLUMN revoked_by text,
    ADD COLUMN revocation_reference text;
ALTER TABLE craft_queue.diagnostic_reads ADD COLUMN grant_id text;
UPDATE craft_queue.diagnostic_reads AS r SET grant_id = g.grant_id
FROM craft_queue.diagnostic_grants AS g
WHERE (r.identity_id, r.case_id, r.policy_digest) = (g.identity_id, g.case_id, g.policy_digest);
ALTER TABLE craft_queue.diagnostic_reads
    ALTER COLUMN grant_id SET NOT NULL,
    DROP CONSTRAINT diagnostic_reads_identity_id_case_id_policy_digest_fkey,
    DROP CONSTRAINT diagnostic_reads_pkey;
ALTER TABLE craft_queue.diagnostic_grants
    DROP CONSTRAINT diagnostic_grants_pkey,
    ADD PRIMARY KEY (grant_id),
    ADD UNIQUE (grant_id, identity_id, case_id, policy_digest);
ALTER TABLE craft_queue.diagnostic_reads
    ADD PRIMARY KEY (grant_id, sequence),
    ADD FOREIGN KEY (grant_id, identity_id, case_id, policy_digest)
        REFERENCES craft_queue.diagnostic_grants (grant_id, identity_id, case_id, policy_digest);
CREATE INDEX diagnostic_grant_scope ON craft_queue.diagnostic_grants
    (identity_id, case_id, policy_digest) WHERE revoked_at IS NULL;

CREATE FUNCTION craft_queue.issue_diagnostic_grant(
    actor text, id text, recipient text, requested_case text, digest text,
    from_sec bigint, to_sec bigint, lifetime_seconds integer, read_limit integer, evidence text
) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    grant_row craft_queue.diagnostic_grants%ROWTYPE;
    now_at timestamptz;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    IF COALESCE(id, '') !~ '^[A-Za-z0-9_-]{1,100}$'
        OR lifetime_seconds IS NULL OR lifetime_seconds NOT BETWEEN 1 AND 2592000
        OR read_limit IS NULL OR read_limit NOT BETWEEN 1 AND 1000
        OR from_sec IS NULL OR from_sec <= 0 OR to_sec IS NULL OR to_sec <= from_sec
        OR length(btrim(COALESCE(evidence, ''))) NOT BETWEEN 1 AND 500 THEN RETURN NULL; END IF;
    SELECT * INTO grant_row FROM craft_queue.diagnostic_grants AS g WHERE g.grant_id = id;
    IF FOUND THEN
        IF grant_row.issued_by = actor AND grant_row.identity_id = recipient
            AND grant_row.case_id = requested_case AND grant_row.policy_digest = digest
            AND grant_row.allowed_from = from_sec AND grant_row.allowed_to = to_sec
            AND grant_row.max_reads = read_limit AND grant_row.approval_reference = evidence
            AND grant_row.expires_at - grant_row.issued_at = make_interval(secs => lifetime_seconds) THEN
            RETURN to_jsonb(grant_row);
        END IF;
        RETURN NULL;
    END IF;

    now_at := clock_timestamp();
    IF EXISTS (SELECT 1 FROM craft_queue.recoveries AS r WHERE r.finished_at IS NULL)
        OR EXISTS (SELECT 1 FROM craft_queue.diagnostic_grants AS g
                   WHERE g.identity_id = recipient AND g.case_id = requested_case AND g.policy_digest = digest
                     AND g.revoked_at IS NULL AND g.expires_at > now_at AND g.reads < g.max_reads) THEN
        RETURN NULL;
    END IF;
    INSERT INTO craft_queue.diagnostic_grants (
        grant_id, identity_id, case_id, policy_digest, allowed_from, allowed_to,
        expires_at, max_reads, approval_reference, issued_by, issued_at
    ) VALUES (
        id, recipient, requested_case, digest, from_sec, to_sec,
        now_at + make_interval(secs => lifetime_seconds), read_limit, evidence, actor, now_at
    ) RETURNING * INTO grant_row;
    RETURN to_jsonb(grant_row);
END;
$$;

CREATE FUNCTION craft_queue.revoke_diagnostic_grant(actor text, id text, evidence text) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    grant_row craft_queue.diagnostic_grants%ROWTYPE;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    IF length(btrim(COALESCE(evidence, ''))) NOT BETWEEN 1 AND 500 THEN RETURN NULL; END IF;
    SELECT * INTO grant_row FROM craft_queue.diagnostic_grants AS g WHERE g.grant_id = id;
    IF NOT FOUND THEN RETURN NULL; END IF;
    IF grant_row.revoked_at IS NOT NULL THEN
        IF grant_row.revoked_by = actor AND grant_row.revocation_reference = evidence THEN
            RETURN to_jsonb(grant_row);
        END IF;
        RETURN NULL;
    END IF;
    UPDATE craft_queue.diagnostic_grants AS g
    SET revoked_at = clock_timestamp(), revoked_by = actor, revocation_reference = evidence
    WHERE g.grant_id = id RETURNING * INTO grant_row;
    RETURN to_jsonb(grant_row);
END;
$$;

CREATE FUNCTION craft_queue.diagnostic_grant_for(
    actor text, requested_case text, digest text, from_sec bigint, to_sec bigint
) RETURNS text
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT g.grant_id FROM craft_queue.diagnostic_grants AS g
    WHERE g.identity_id = actor AND g.case_id = requested_case AND g.policy_digest = digest
      AND g.revoked_at IS NULL AND g.expires_at > statement_timestamp() AND g.reads < g.max_reads
      AND g.allowed_from <= from_sec AND to_sec <= g.allowed_to AND from_sec < to_sec
    ORDER BY g.issued_at DESC NULLS LAST, g.grant_id LIMIT 1;
$$;

CREATE FUNCTION craft_queue.consume_diagnostic_grant(
    id text, actor text, requested_case text, digest text, from_sec bigint, to_sec bigint
) RETURNS boolean
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    WITH permitted AS (
        UPDATE craft_queue.diagnostic_grants AS g SET reads = reads + 1
        WHERE g.grant_id = id AND g.identity_id = actor AND g.case_id = requested_case AND g.policy_digest = digest
          AND g.revoked_at IS NULL AND g.expires_at > clock_timestamp() AND g.reads < g.max_reads
          AND g.allowed_from <= from_sec AND to_sec <= g.allowed_to AND from_sec < to_sec
        RETURNING g.*
    ), receipt AS (
        INSERT INTO craft_queue.diagnostic_reads (
            grant_id, identity_id, case_id, policy_digest, sequence, requested_from, requested_to,
            approval_reference, authorized_at
        )
        SELECT p.grant_id, p.identity_id, p.case_id, p.policy_digest, p.reads, from_sec, to_sec,
               p.approval_reference, clock_timestamp()
        FROM permitted AS p RETURNING sequence
    )
    SELECT EXISTS (SELECT 1 FROM receipt);
$$;

CREATE OR REPLACE FUNCTION craft_queue.diagnostic_allowed(
    actor text, requested_case text, digest text, from_sec bigint, to_sec bigint
) RETURNS boolean LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT craft_queue.diagnostic_grant_for(actor, requested_case, digest, from_sec, to_sec) IS NOT NULL;
$$;

CREATE OR REPLACE FUNCTION craft_queue.consume_diagnostic(
    actor text, requested_case text, digest text, from_sec bigint, to_sec bigint
) RETURNS boolean LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT craft_queue.consume_diagnostic_grant(
        craft_queue.diagnostic_grant_for(actor, requested_case, digest, from_sec, to_sec),
        actor, requested_case, digest, from_sec, to_sec);
$$;

CREATE OR REPLACE FUNCTION craft_queue.revoke_diagnostics_on_restore() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog AS $$
BEGIN
    UPDATE craft_queue.diagnostic_grants SET revoked_at = NEW.started_at,
        revoked_by = NEW.started_by, revocation_reference = 'restore:' || NEW.epoch::text
    WHERE revoked_at IS NULL;
    RETURN NEW;
END;
$$;

CREATE FUNCTION craft_queue.diagnostic_grant_audit(id text) RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT jsonb_build_object('grant', to_jsonb(g), 'receipts', COALESCE((
        SELECT jsonb_agg(to_jsonb(recent) ORDER BY recent.sequence DESC)
        FROM (SELECT sequence, requested_from, requested_to, authorized_at
              FROM craft_queue.diagnostic_reads AS r WHERE r.grant_id = id
              ORDER BY sequence DESC LIMIT 50) AS recent
    ), '[]'::jsonb))
    FROM craft_queue.diagnostic_grants AS g WHERE g.grant_id = id;
$$;

REVOKE ALL ON FUNCTION craft_queue.issue_diagnostic_grant(text, text, text, text, text, bigint, bigint, integer, integer, text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.revoke_diagnostic_grant(text, text, text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.diagnostic_grant_for(text, text, text, bigint, bigint) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.consume_diagnostic_grant(text, text, text, text, bigint, bigint) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.diagnostic_grant_audit(text) FROM PUBLIC;

COMMIT;
