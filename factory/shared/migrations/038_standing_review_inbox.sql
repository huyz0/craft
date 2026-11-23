BEGIN;

CREATE OR REPLACE FUNCTION craft_queue.review_inbox(repo text, owner_name text, requested_limit integer, reviewer text) RETURNS jsonb
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
                WHEN standing.delegation_blocked THEN 'check_standing_delegation'
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
        CROSS JOIN LATERAL (
            SELECT w.state IN ('queued', 'leased', 'candidate')
                AND NOT craft_queue.standing_delegation_current(w.id, statement_timestamp())
                AS delegation_blocked
        ) AS standing
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
                OR standing.delegation_blocked
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
                WHEN 'check_standing_delegation' THEN 4
                WHEN 'inspect_draft' THEN 5
                WHEN 'verify_candidate' THEN 6
                WHEN 'wait_for_predecessor' THEN 8
                ELSE 7
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

COMMIT;
