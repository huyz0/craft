BEGIN;

CREATE FUNCTION craft_queue.pending_standalone_merge(order_id text, generation_id bigint,
    candidate_head text, pack_pin text, verification_id bigint, publication_id text) RETURNS jsonb
LANGUAGE sql STABLE SET search_path = pg_catalog AS $$
    SELECT CASE WHEN publication_id = craft_queue.current_review_draft(order_id, generation_id, candidate_head, verification_id)
        AND NOT EXISTS (SELECT 1 FROM craft_queue.series_members AS s WHERE s.work_id = order_id)
        AND NOT EXISTS (SELECT 1 FROM craft_queue.recoveries AS r WHERE r.finished_at IS NULL)
        AND NOT EXISTS (SELECT 1 FROM craft_queue.publications AS p WHERE p.work_id = order_id
                        AND p.state IN ('reserved', 'sending', 'uncertain'))
        THEN COALESCE(
            (SELECT jsonb_build_object('forge', 'github', 'merge_receipt_id', r.id,
                                       'result_commit', m.observation->>'result_commit')
             FROM craft_queue.shadow_merge_receipts AS r
             JOIN craft_queue.github_shadow_merges AS m ON m.receipt_id = r.id
             WHERE r.work_id = order_id AND r.generation = generation_id AND r.head = candidate_head
               AND r.pack_digest = pack_pin AND r.publication_key = publication_id
               AND m.observation->>'head' = candidate_head
               AND m.observation->>'result_commit' ~ '^([a-f0-9]{40}|[a-f0-9]{64})$'
             ORDER BY m.recorded_at DESC, r.id LIMIT 1),
            (SELECT jsonb_build_object('forge', 'bitbucket', 'publication_key', m.publication_key,
                                       'result_commit', m.observation->>'result_commit')
             FROM craft_queue.bitbucket_work_merges AS m
             WHERE m.work_id = order_id AND m.publication_key = publication_id AND m.closed_at IS NULL
               AND m.observation->>'head' = candidate_head)
        ) END;
$$;

REVOKE ALL ON FUNCTION craft_queue.pending_standalone_merge(text, bigint, text, text, bigint, text) FROM PUBLIC;

CREATE OR REPLACE FUNCTION craft_queue.review_inbox_before_repair_waits(repo text, owner_name text, requested_limit integer, reviewer text) RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    WITH scope AS MATERIALIZED (
        SELECT COALESCE(array_agg(g.repository), ARRAY[]::text[]) AS repos
        FROM craft_queue.review_inbox_grants AS g
        WHERE g.reviewer_id = $4 AND g.revoked_at IS NULL AND g.expires_at > statement_timestamp()
    ), entries AS MATERIALIZED (
        SELECT w.id AS work_id, w.repository, left(w.payload->>'issue', 100) AS issue,
            left(w.payload->>'owner', 200) AS owner, left(w.payload->>'intent', 240) AS intent_excerpt,
            COALESCE(acceptance.excerpts, '[]'::jsonb) AS acceptance_excerpts,
            w.created_at, w.state, w.generation, w.candidate AS head, w.candidate_evidence AS pack_digest,
            (w.payload->'require_native_checks' IS NOT DISTINCT FROM 'true'::jsonb) AS requires_native_checks,
            to_jsonb(verdict) AS independent_verification, to_jsonb(publication) AS publication,
            repair.native_url AS prior_draft_url, predecessor.id IS NOT NULL AS dependency_blocked,
            to_jsonb(change_request) AS pending_repair, merge.closure AS merge_closure,
            CASE WHEN $4 IS NULL OR dependency.repository = ANY(scope.repos)
                THEN predecessor.id END AS waiting_on,
            CASE
                WHEN publication.state IN ('sending', 'uncertain') THEN 'reconcile_publication'
                WHEN merge.closure IS NOT NULL THEN 'close_merged_work'
                WHEN standing.delegation_blocked THEN 'check_standing_delegation'
                WHEN w.state <> 'candidate' THEN 'resolve_attempt_cap'
                WHEN verdict.sequence IS NULL THEN 'verify_candidate'
                WHEN NOT verdict.passed THEN 'resolve_failed_verification'
                WHEN predecessor.id IS NOT NULL THEN 'wait_for_predecessor'
                WHEN change_request.handoff_id IS NOT NULL THEN 'resolve_review_changes'
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
        LEFT JOIN LATERAL (
            SELECT CASE WHEN $4 IS NULL AND publication.state = 'confirmed' AND verdict.passed
                THEN craft_queue.pending_standalone_merge(
                    w.id, w.generation, w.candidate, w.candidate_evidence, verdict.sequence, publication.action_key
                ) END AS closure
        ) AS merge ON true
        LEFT JOIN LATERAL (
            SELECT h.id AS handoff_id, h.reviewer_id, h.decision_evidence AS evidence,
                left(h.decision_note, 240) AS feedback_excerpt, h.automatic_repair,
                count(*) OVER () AS decision_count,
                CASE WHEN settings.paused THEN 'queue_paused'
                    WHEN w.payload->>'execution_profile' IS NOT NULL
                        AND w.generation >= (w.payload->'execution_profile'->>'max_attempts')::bigint
                        THEN 'attempt_cap'
                END AS blocker
            FROM craft_queue.review_handoffs AS h CROSS JOIN craft_queue.control AS settings
            WHERE $4 IS NULL AND h.work_id = w.id AND h.generation = w.generation
              AND h.head = w.candidate AND h.decision = 'changes_requested'
              AND h.publication_key = craft_queue.current_review_draft(
                  w.id, w.generation, w.candidate, h.verification_sequence
              )
              AND NOT EXISTS (
                  SELECT 1 FROM craft_queue.repair_requests AS r
                  WHERE r.work_id = w.id AND r.from_generation = w.generation
              )
            ORDER BY h.decided_at, h.id LIMIT 1
        ) AS change_request ON true
        CROSS JOIN LATERAL (SELECT craft_queue.series_blocker(w.id) AS id) AS predecessor
        LEFT JOIN craft_queue.work AS dependency ON dependency.id = predecessor.id
        CROSS JOIN LATERAL (
            SELECT jsonb_agg(left(item.value #>> '{}', 160) ORDER BY item.position) AS excerpts
            FROM jsonb_array_elements(CASE WHEN jsonb_typeof(w.payload->'acceptance') = 'array'
                THEN w.payload->'acceptance' ELSE '[]'::jsonb END) WITH ORDINALITY AS item(value, position)
            WHERE item.position <= 3 AND jsonb_typeof(item.value) = 'string'
        ) AS acceptance
        WHERE ($4 IS NULL OR w.repository = ANY(scope.repos))
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
                WHEN 'reconcile_publication' THEN 0
                WHEN 'close_merged_work' THEN 1
                WHEN 'resolve_failed_verification' THEN 2
                WHEN 'resolve_attempt_cap' THEN 3
                WHEN 'check_standing_delegation' THEN 4
                WHEN 'resolve_review_changes' THEN 5
                WHEN 'inspect_draft' THEN 6
                WHEN 'verify_candidate' THEN 7
                WHEN 'wait_for_predecessor' THEN 9
                ELSE 8
            END, created_at, work_id) AS priority
        FROM entries
    )
    SELECT jsonb_build_object(
        'total', (SELECT count(*) FROM entries),
        'counts', COALESCE((SELECT jsonb_object_agg(stage.next_action, stage.total)
            FROM (SELECT next_action, count(*) AS total FROM entries GROUP BY next_action) AS stage), '{}'::jsonb),
        'items', COALESCE((SELECT jsonb_agg((to_jsonb(ordered) - 'priority' - 'pending_repair' - 'merge_closure')
            || CASE WHEN merge_closure IS NOT NULL
                THEN jsonb_build_object('merge_closure', merge_closure) ELSE '{}'::jsonb END
            || CASE WHEN next_action = 'resolve_review_changes'
                THEN jsonb_build_object('pending_repair', pending_repair) ELSE '{}'::jsonb END
            ORDER BY priority)
            FROM ordered WHERE priority <= least(greatest(requested_limit, 1), 50)), '[]'::jsonb)
    );
$$;

COMMIT;
