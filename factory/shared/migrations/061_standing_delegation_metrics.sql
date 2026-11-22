BEGIN;

CREATE OR REPLACE FUNCTION craft_queue.operation_summary() RETURNS jsonb
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT sample.summary || jsonb_build_object(
        'queued_standing', standing.queued,
        'review_reserved', (sample.summary->>'review_reserved')::bigint + standing.queued,
        'standing_delegation_blocked', standing.blocked
    )
    FROM (SELECT craft_queue.operation_summary_before_standing() AS summary) AS sample
    CROSS JOIN (
        SELECT count(*) FILTER (WHERE w.state = 'queued' AND w.repair_from_generation IS NULL) AS queued,
            count(*) FILTER (WHERE NOT craft_queue.standing_delegation_current(w.id, statement_timestamp())) AS blocked
        FROM craft_queue.work AS w
        JOIN craft_queue.standing_work AS binding ON binding.work_id = w.id
        WHERE w.state IN ('queued', 'leased', 'candidate')
    ) AS standing;
$$;

COMMIT;
