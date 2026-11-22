BEGIN;

CREATE TABLE craft_queue.retired_diagnostic_cases (
    case_id text NOT NULL,
    policy_digest text NOT NULL,
    retired_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    retired_by text NOT NULL,
    evidence text NOT NULL,
    replacement_catalog_digest text NOT NULL,
    PRIMARY KEY (case_id, policy_digest)
);

CREATE OR REPLACE FUNCTION craft_queue.issue_diagnostic_grant(
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
    IF EXISTS (SELECT 1 FROM craft_queue.retired_diagnostic_cases AS retired
        WHERE retired.case_id = requested_case AND retired.policy_digest = digest) THEN
        RETURN NULL;
    END IF;
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

CREATE FUNCTION craft_queue.retire_diagnostic_cases(
    actor_id text, catalog_digest text, previous_pins jsonb, next_pins jsonb, evidence_ref text
) RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    retired_count integer;
    revoked_count integer;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    IF EXISTS (SELECT 1 FROM craft_queue.recoveries WHERE finished_at IS NULL)
        OR EXISTS (SELECT 1 FROM jsonb_each_text(next_pins) AS candidate(case_id, digest)
            JOIN craft_queue.retired_diagnostic_cases AS retired
            ON retired.case_id = candidate.case_id AND retired.policy_digest = candidate.digest) THEN
        RETURN NULL;
    END IF;

    INSERT INTO craft_queue.retired_diagnostic_cases (
        case_id, policy_digest, retired_by, evidence, replacement_catalog_digest
    )
    SELECT old.case_id, old.digest, actor_id, evidence_ref, catalog_digest
    FROM jsonb_each_text(previous_pins) AS old(case_id, digest)
    WHERE next_pins ->> old.case_id IS DISTINCT FROM old.digest
    ON CONFLICT DO NOTHING;
    GET DIAGNOSTICS retired_count = ROW_COUNT;

    UPDATE craft_queue.diagnostic_grants AS grant_row
    SET revoked_at = clock_timestamp(), revoked_by = actor_id, revocation_reference = evidence_ref
    FROM jsonb_each_text(previous_pins) AS old(case_id, digest)
    WHERE grant_row.case_id = old.case_id AND grant_row.policy_digest = old.digest
        AND next_pins ->> old.case_id IS DISTINCT FROM old.digest AND grant_row.revoked_at IS NULL;
    GET DIAGNOSTICS revoked_count = ROW_COUNT;
    RETURN jsonb_build_object('retired_cases', retired_count, 'revoked_grants', revoked_count);
END;
$$;

REVOKE ALL ON FUNCTION craft_queue.retire_diagnostic_cases(text,text,jsonb,jsonb,text) FROM PUBLIC;

COMMIT;
