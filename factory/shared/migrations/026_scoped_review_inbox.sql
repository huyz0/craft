BEGIN;

CREATE TABLE craft_queue.review_inbox_grants (
    grant_id text PRIMARY KEY,
    reviewer_id text NOT NULL,
    repository text NOT NULL,
    approved_by text NOT NULL,
    approval_reference text NOT NULL,
    granted_at timestamptz NOT NULL,
    expires_at timestamptz NOT NULL CHECK (expires_at > granted_at),
    revoked_at timestamptz,
    revoked_by text,
    revocation_reference text
);
CREATE INDEX review_inbox_grants_active ON craft_queue.review_inbox_grants (reviewer_id, repository, expires_at)
WHERE revoked_at IS NULL;

CREATE FUNCTION craft_queue.grant_review_inbox(
    actor text, id text, reviewer text, repo text, evidence text, lifetime_seconds integer
) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    grant_row craft_queue.review_inbox_grants%ROWTYPE;
    now_at timestamptz;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    IF COALESCE(id, '') !~ '^[A-Za-z0-9_-]{1,100}$'
        OR COALESCE(reviewer, '') !~ '^[A-Za-z0-9_-]{1,100}$'
        OR length(btrim(COALESCE(repo, ''))) NOT BETWEEN 1 AND 200
        OR length(btrim(COALESCE(evidence, ''))) NOT BETWEEN 1 AND 500
        OR lifetime_seconds IS NULL OR lifetime_seconds NOT BETWEEN 1 AND 2592000 THEN RETURN NULL; END IF;
    SELECT * INTO grant_row FROM craft_queue.review_inbox_grants AS g WHERE g.grant_id = id;
    IF FOUND THEN
        IF grant_row.reviewer_id = reviewer AND grant_row.repository = repo
            AND grant_row.approved_by = actor AND grant_row.approval_reference = evidence
            AND grant_row.expires_at - grant_row.granted_at = make_interval(secs => lifetime_seconds) THEN
            RETURN to_jsonb(grant_row);
        END IF;
        RETURN NULL;
    END IF;
    now_at := clock_timestamp();
    IF EXISTS (SELECT 1 FROM craft_queue.recoveries AS r WHERE r.finished_at IS NULL)
        OR EXISTS (SELECT 1 FROM craft_queue.review_inbox_grants AS g
            WHERE g.reviewer_id = reviewer AND g.repository = repo
            AND g.revoked_at IS NULL AND g.expires_at > now_at) THEN RETURN NULL; END IF;
    INSERT INTO craft_queue.review_inbox_grants (grant_id, reviewer_id, repository, approved_by,
        approval_reference, granted_at, expires_at)
    VALUES (id, reviewer, repo, actor, evidence, now_at, now_at + make_interval(secs => lifetime_seconds))
    RETURNING * INTO grant_row;
    RETURN to_jsonb(grant_row);
END;
$$;

CREATE FUNCTION craft_queue.revoke_review_inbox(actor text, id text, evidence text) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    grant_row craft_queue.review_inbox_grants%ROWTYPE;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    IF length(btrim(COALESCE(evidence, ''))) NOT BETWEEN 1 AND 500 THEN RETURN NULL; END IF;
    SELECT * INTO grant_row FROM craft_queue.review_inbox_grants AS g WHERE g.grant_id = id;
    IF NOT FOUND THEN RETURN NULL; END IF;
    IF grant_row.revoked_at IS NOT NULL THEN
        IF grant_row.revoked_by = actor AND grant_row.revocation_reference = evidence THEN
            RETURN to_jsonb(grant_row);
        END IF;
        RETURN NULL;
    END IF;
    UPDATE craft_queue.review_inbox_grants AS g
    SET revoked_at = clock_timestamp(), revoked_by = actor, revocation_reference = evidence
    WHERE g.grant_id = id RETURNING * INTO grant_row;
    RETURN to_jsonb(grant_row);
END;
$$;

CREATE FUNCTION craft_queue.revoke_review_inbox_on_restore() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
BEGIN
    UPDATE craft_queue.review_inbox_grants SET revoked_at = NEW.started_at, revoked_by = NEW.started_by,
        revocation_reference = 'restore:' || NEW.epoch::text
    WHERE revoked_at IS NULL;
    RETURN NEW;
END;
$$;
CREATE TRIGGER revoke_review_inbox_on_restore AFTER INSERT ON craft_queue.recoveries
FOR EACH ROW EXECUTE FUNCTION craft_queue.revoke_review_inbox_on_restore();

CREATE FUNCTION craft_queue.review_inbox(repo text, owner_name text, requested_limit integer, reviewer text) RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    WITH scope AS MATERIALIZED (
        SELECT COALESCE(array_agg(g.repository), ARRAY[]::text[]) AS repos
        FROM craft_queue.review_inbox_grants AS g
        WHERE g.reviewer_id = reviewer AND g.revoked_at IS NULL AND g.expires_at > statement_timestamp()
    ), entries AS MATERIALIZED (
        SELECT w.id AS work_id, w.repository, left(w.payload->>'issue', 100) AS issue,
            left(w.payload->>'owner', 200) AS owner, left(w.payload->>'intent', 240) AS intent_excerpt,
            COALESCE(acceptance.excerpts, '[]'::jsonb) AS acceptance_excerpts,
            w.created_at, w.state, w.generation, w.candidate AS head, w.candidate_evidence AS pack_digest,
            (w.payload->'require_native_checks' IS NOT DISTINCT FROM 'true'::jsonb) AS requires_native_checks,
            to_jsonb(verdict) AS independent_verification, to_jsonb(publication) AS publication,
            repair.native_url AS prior_draft_url, predecessor.id IS NOT NULL AS dependency_blocked,
            CASE WHEN reviewer IS NULL OR dependency.repository = ANY(scope.repos)
                THEN predecessor.id END AS waiting_on,
            CASE
                WHEN publication.state IN ('sending', 'uncertain') THEN 'reconcile_publication'
                WHEN w.state <> 'candidate' THEN 'resolve_attempt_cap'
                WHEN verdict.sequence IS NULL THEN 'verify_candidate'
                WHEN NOT verdict.passed THEN 'resolve_failed_verification'
                WHEN predecessor.id IS NOT NULL THEN 'wait_for_predecessor'
                WHEN publication.state = 'confirmed' THEN 'inspect_draft'
                WHEN publication.state = 'reserved' THEN 'inspect_reservation'
                WHEN publication.state = 'absent' THEN 'inspect_absent_publication'
                ELSE 'choose_publication'
            END AS next_action
        FROM craft_queue.work AS w CROSS JOIN scope
        LEFT JOIN craft_queue.attempts AS a ON a.work_id = w.id AND a.generation = w.generation
        LEFT JOIN craft_queue.repair_requests AS repair
            ON repair.work_id = w.id AND repair.from_generation = w.repair_from_generation
        LEFT JOIN LATERAL (
            SELECT v.sequence, v.verifier, v.passed, v.evidence, v.observed_at
            FROM craft_queue.verifications AS v
            WHERE v.work_id = w.id AND v.generation = w.generation
                AND v.head = w.candidate AND v.digest = w.candidate_evidence
            ORDER BY v.sequence DESC LIMIT 1
        ) AS verdict ON w.state = 'candidate'
        LEFT JOIN LATERAL (
            SELECT p.action_key, p.generation, p.head, p.state, p.result_url, p.verification_sequence
            FROM craft_queue.publications AS p
            WHERE p.work_id = w.id AND (
                p.state IN ('sending', 'uncertain')
                OR (p.generation = w.generation AND p.head = w.candidate AND p.pack_digest = w.candidate_evidence)
            )
            ORDER BY (p.state IN ('sending', 'uncertain')) DESC, p.generation DESC LIMIT 1
        ) AS publication ON true
        CROSS JOIN LATERAL (SELECT craft_queue.series_blocker(w.id) AS id) AS predecessor
        LEFT JOIN craft_queue.work AS dependency ON dependency.id = predecessor.id
        CROSS JOIN LATERAL (
            SELECT jsonb_agg(left(item.value #>> '{}', 160) ORDER BY item.position) AS excerpts
            FROM jsonb_array_elements(CASE WHEN jsonb_typeof(w.payload->'acceptance') = 'array'
                THEN w.payload->'acceptance' ELSE '[]'::jsonb END) WITH ORDINALITY AS item(value, position)
            WHERE item.position <= 3 AND jsonb_typeof(item.value) = 'string'
        ) AS acceptance
        WHERE (reviewer IS NULL OR w.repository = ANY(scope.repos))
            AND (repo IS NULL OR w.repository = repo)
            AND (owner_name IS NULL OR w.payload->>'owner' = owner_name)
            AND (
                w.state = 'candidate' OR publication.state IN ('sending', 'uncertain')
                OR (w.payload->>'execution_profile' IS NOT NULL
                    AND w.generation >= (w.payload->'execution_profile'->>'max_attempts')::bigint
                    AND (w.state = 'queued' OR (w.state = 'leased' AND a.expires_at <= statement_timestamp())))
            )
    ), ordered AS (
        SELECT entries.*, row_number() OVER (ORDER BY
            CASE next_action
                WHEN 'reconcile_publication' THEN 1
                WHEN 'resolve_failed_verification' THEN 2
                WHEN 'resolve_attempt_cap' THEN 3
                WHEN 'inspect_draft' THEN 4
                WHEN 'verify_candidate' THEN 5
                WHEN 'wait_for_predecessor' THEN 7
                ELSE 6
            END, created_at, work_id) AS priority
        FROM entries
    )
    SELECT jsonb_build_object(
        'total', (SELECT count(*) FROM entries),
        'counts', COALESCE((SELECT jsonb_object_agg(stage.next_action, stage.total)
            FROM (SELECT next_action, count(*) AS total FROM entries GROUP BY next_action) AS stage), '{}'::jsonb),
        'items', COALESCE((SELECT jsonb_agg(to_jsonb(ordered) - 'priority' ORDER BY priority)
            FROM ordered WHERE priority <= least(greatest(requested_limit, 1), 50)), '[]'::jsonb)
    );
$$;

CREATE OR REPLACE FUNCTION craft_queue.review_inbox(repo text, owner_name text, requested_limit integer) RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT craft_queue.review_inbox(repo, owner_name, requested_limit, NULL::text);
$$;

REVOKE ALL ON FUNCTION craft_queue.grant_review_inbox(text, text, text, text, text, integer) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.revoke_review_inbox(text, text, text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.revoke_review_inbox_on_restore() FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.review_inbox(text, text, integer, text) FROM PUBLIC;
COMMIT;
