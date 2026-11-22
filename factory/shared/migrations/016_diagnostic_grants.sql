BEGIN;

CREATE TABLE craft_queue.diagnostic_grants (
    identity_id text NOT NULL CHECK (identity_id ~ '^[A-Za-z0-9_-]{1,100}$'),
    case_id text NOT NULL CHECK (case_id ~ '^[A-Za-z0-9_-]{1,100}$'),
    policy_digest text NOT NULL CHECK (policy_digest ~ '^sha256:[a-f0-9]{64}$'),
    allowed_from bigint NOT NULL CHECK (allowed_from > 0),
    allowed_to bigint NOT NULL CHECK (allowed_to > allowed_from),
    expires_at timestamptz NOT NULL,
    revoked_at timestamptz,
    max_reads integer NOT NULL CHECK (max_reads BETWEEN 1 AND 1000),
    reads integer NOT NULL DEFAULT 0 CHECK (reads BETWEEN 0 AND max_reads),
    approval_reference text NOT NULL CHECK (length(btrim(approval_reference)) BETWEEN 1 AND 500),
    PRIMARY KEY (identity_id, case_id, policy_digest)
);

CREATE TABLE craft_queue.diagnostic_reads (
    identity_id text NOT NULL,
    case_id text NOT NULL,
    policy_digest text NOT NULL,
    sequence integer NOT NULL CHECK (sequence > 0),
    requested_from bigint NOT NULL,
    requested_to bigint NOT NULL,
    approval_reference text NOT NULL,
    authorized_at timestamptz NOT NULL,
    PRIMARY KEY (identity_id, case_id, policy_digest, sequence),
    FOREIGN KEY (identity_id, case_id, policy_digest)
        REFERENCES craft_queue.diagnostic_grants(identity_id, case_id, policy_digest)
);

CREATE FUNCTION craft_queue.diagnostic_allowed(
    actor text, requested_case text, digest text, from_sec bigint, to_sec bigint
) RETURNS boolean
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT EXISTS (
        SELECT 1 FROM craft_queue.diagnostic_grants AS g
        WHERE g.identity_id = actor AND g.case_id = requested_case AND g.policy_digest = digest
          AND g.revoked_at IS NULL AND g.expires_at > statement_timestamp() AND g.reads < g.max_reads
          AND g.allowed_from <= from_sec AND to_sec <= g.allowed_to AND from_sec < to_sec
    );
$$;

CREATE FUNCTION craft_queue.consume_diagnostic(
    actor text, requested_case text, digest text, from_sec bigint, to_sec bigint
) RETURNS boolean
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    WITH permitted AS (
        UPDATE craft_queue.diagnostic_grants AS g SET reads = reads + 1
        WHERE g.identity_id = actor AND g.case_id = requested_case AND g.policy_digest = digest
          AND g.revoked_at IS NULL AND g.expires_at > clock_timestamp() AND g.reads < g.max_reads
          AND g.allowed_from <= from_sec AND to_sec <= g.allowed_to AND from_sec < to_sec
        RETURNING g.*
    ), receipt AS (
        INSERT INTO craft_queue.diagnostic_reads (
            identity_id, case_id, policy_digest, sequence, requested_from, requested_to,
            approval_reference, authorized_at
        )
        SELECT p.identity_id, p.case_id, p.policy_digest, p.reads, from_sec, to_sec,
               p.approval_reference, clock_timestamp()
        FROM permitted AS p RETURNING sequence
    )
    SELECT EXISTS (SELECT 1 FROM receipt);
$$;

REVOKE ALL ON FUNCTION craft_queue.diagnostic_allowed(text, text, text, bigint, bigint) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.consume_diagnostic(text, text, text, bigint, bigint) FROM PUBLIC;

CREATE FUNCTION craft_queue.revoke_diagnostics_on_restore() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog AS $$
BEGIN
    UPDATE craft_queue.diagnostic_grants SET revoked_at = clock_timestamp() WHERE revoked_at IS NULL;
    RETURN NEW;
END;
$$;

CREATE TRIGGER revoke_diagnostics_on_restore AFTER INSERT ON craft_queue.recoveries
FOR EACH ROW EXECUTE FUNCTION craft_queue.revoke_diagnostics_on_restore();
REVOKE ALL ON FUNCTION craft_queue.revoke_diagnostics_on_restore() FROM PUBLIC;
COMMIT;
