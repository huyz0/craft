BEGIN;

ALTER TABLE craft_queue.retired_diagnostic_cases
    ALTER COLUMN replacement_catalog_digest DROP NOT NULL;

CREATE INDEX diagnostic_grants_case_listing ON craft_queue.diagnostic_grants (case_id, grant_id);

CREATE FUNCTION craft_queue.inspect_diagnostic_case_access(
    requested_case text, digest text, after_id text, page_size integer
) RETURNS jsonb LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    WITH matches AS MATERIALIZED (
        SELECT g.grant_id, g.identity_id, g.policy_digest, g.allowed_from, g.allowed_to,
               g.expires_at, g.max_reads, g.reads, g.revoked_at,
               (g.revoked_at IS NULL AND g.expires_at > statement_timestamp() AND g.reads < g.max_reads) AS active
        FROM craft_queue.diagnostic_grants AS g
        WHERE g.case_id = requested_case AND (digest IS NULL OR g.policy_digest = digest)
          AND (after_id IS NULL OR g.grant_id > after_id)
        ORDER BY g.grant_id LIMIT (page_size + 1)
    ), page AS (
        SELECT * FROM matches ORDER BY grant_id LIMIT page_size
    )
    SELECT jsonb_build_object(
        'case_id', requested_case, 'policy_digest', digest,
        'grants', COALESCE((SELECT jsonb_agg(to_jsonb(page) ORDER BY grant_id) FROM page), '[]'::jsonb),
        'next_after', CASE WHEN (SELECT count(*) FROM matches) > page_size THEN
            (SELECT max(grant_id) FROM page) END,
        'retirement', (SELECT to_jsonb(r) FROM craft_queue.retired_diagnostic_cases AS r
            WHERE r.case_id = requested_case AND r.policy_digest = digest)
    );
$$;

CREATE FUNCTION craft_queue.retire_diagnostic_case_access(
    actor_id text, requested_case text, digest text, evidence_ref text
) RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    retired jsonb;
BEGIN
    retired := craft_queue.retire_diagnostic_cases(
        actor_id, NULL, jsonb_build_object(requested_case, digest), '{}'::jsonb, evidence_ref);
    IF retired IS NULL THEN RETURN NULL; END IF;
    RETURN retired || jsonb_build_object('retirement', (
        SELECT to_jsonb(r) FROM craft_queue.retired_diagnostic_cases AS r
        WHERE r.case_id = requested_case AND r.policy_digest = digest
    ));
END;
$$;

REVOKE ALL ON FUNCTION craft_queue.inspect_diagnostic_case_access(text,text,text,integer) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.retire_diagnostic_case_access(text,text,text,text) FROM PUBLIC;

COMMIT;
