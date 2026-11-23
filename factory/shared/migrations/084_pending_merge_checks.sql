BEGIN;

CREATE INDEX work_merge_check_candidates ON craft_queue.work (repository, id COLLATE "C")
    WHERE state = 'candidate';

CREATE FUNCTION craft_queue.pending_merge_checks(actor_id text, repo text, after_id text, requested_limit integer)
RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    WITH page AS MATERIALIZED (
        SELECT w.id AS work_id, w.repository, w.candidate AS head, p.result_url AS pull_request,
               p.base_branch, v.sequence AS verification_sequence, result.fields
        FROM craft_queue.work AS w
        JOIN craft_queue.attempts AS a ON a.work_id = w.id AND a.generation = w.generation
        JOIN LATERAL (
            SELECT latest.sequence FROM craft_queue.verifications AS latest
            WHERE latest.work_id = w.id AND latest.generation = w.generation
            ORDER BY latest.sequence DESC LIMIT 1
        ) AS v ON true
        JOIN craft_queue.publications AS p ON p.action_key =
            craft_queue.current_review_draft(w.id, w.generation, w.candidate, v.sequence)
        LEFT JOIN craft_queue.series_members AS series ON series.work_id = w.id
        CROSS JOIN LATERAL (
            SELECT CASE
                WHEN p.result_url LIKE 'https://github.com/%'
                  AND (series.work_id IS NULL OR series.requires_rollout_proof)
                THEN (
                    SELECT jsonb_build_object(
                        'forge', 'github', 'receipt_id', r.id, 'report_digest', r.report_digest,
                        'policy_file_digest', r.assessment #>> '{github,policy_file_digest}'
                    )
                    FROM craft_queue.shadow_merge_receipts AS r
                    WHERE r.work_id = w.id AND r.generation = w.generation AND r.head = w.candidate
                      AND r.pack_digest = w.candidate_evidence AND r.publication_key = p.action_key
                      AND r.assessment ? 'github' AND r.assessment->>'pull_request' = p.result_url
                      AND r.actor <> actor_id
                      AND (series.work_id IS NULL OR r.verification_sequence = v.sequence)
                      AND NOT EXISTS (
                          SELECT 1 FROM craft_queue.shadow_merge_receipts AS observed
                          JOIN craft_queue.github_shadow_merges AS m ON m.receipt_id = observed.id
                          WHERE observed.work_id = w.id AND observed.generation = w.generation
                            AND observed.head = w.candidate AND observed.pack_digest = w.candidate_evidence
                            AND observed.publication_key = p.action_key
                            AND (series.work_id IS NULL OR observed.verification_sequence = v.sequence)
                      )
                    ORDER BY (r.assessment->>'as_of')::bigint, r.id COLLATE "C" LIMIT 1
                )
                WHEN p.result_url LIKE 'https://bitbucket.org/%'
                THEN (
                    SELECT jsonb_build_object('forge', 'bitbucket', 'publication_key', target.publication_key)
                    FROM craft_queue.bitbucket_closure_target(w.id, w.candidate) AS target
                    WHERE NOT EXISTS (SELECT 1 FROM craft_queue.bitbucket_work_merges AS m
                                      WHERE m.publication_key = target.publication_key)
                )
            END AS fields
        ) AS result
        WHERE w.repository = repo AND w.state = 'candidate' AND a.runner_id <> actor_id
          AND (after_id IS NULL OR w.id COLLATE "C" > after_id)
          AND NOT EXISTS (SELECT 1 FROM craft_queue.recoveries AS recovery WHERE recovery.finished_at IS NULL)
          AND NOT EXISTS (SELECT 1 FROM craft_queue.publications AS active WHERE active.work_id = w.id
                          AND active.state IN ('reserved', 'sending', 'uncertain'))
          AND result.fields IS NOT NULL
        ORDER BY w.id COLLATE "C" LIMIT least(greatest(requested_limit, 1), 50) + 1
    ), returned AS MATERIALIZED (
        SELECT page.* FROM page ORDER BY work_id COLLATE "C" LIMIT least(greatest(requested_limit, 1), 50)
    )
    SELECT jsonb_build_object('repository', repo, 'items', COALESCE(
        (SELECT jsonb_agg((to_jsonb(returned) - 'fields') || fields ORDER BY work_id COLLATE "C") FROM returned),
        '[]'::jsonb
    ), 'next_after_work_id', CASE WHEN (SELECT count(*) FROM page) > least(greatest(requested_limit, 1), 50)
        THEN (SELECT max(work_id COLLATE "C") FROM returned) END);
$$;

REVOKE ALL ON FUNCTION craft_queue.pending_merge_checks(text, text, text, integer) FROM PUBLIC;

COMMIT;
