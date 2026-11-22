BEGIN;

ALTER TABLE craft_queue.github_work_closures
    ADD COLUMN verification_sequence bigint REFERENCES craft_queue.verifications(sequence);

UPDATE craft_queue.github_work_closures AS c
SET verification_sequence = r.verification_sequence
FROM craft_queue.shadow_merge_receipts AS r
WHERE r.id = c.merge_receipt_id;

ALTER TABLE craft_queue.github_work_closures ALTER COLUMN verification_sequence SET NOT NULL;

CREATE OR REPLACE FUNCTION craft_queue.close_github_work(
    actor_id text, order_id text, receipt text, expected_head text, evidence_ref text
) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    closure craft_queue.github_work_closures%ROWTYPE;
    result text;
    verified_sequence bigint;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    SELECT c.* INTO closure FROM craft_queue.github_work_closures AS c WHERE c.work_id = order_id;
    IF FOUND THEN
        IF closure.merge_receipt_id = receipt AND closure.head = expected_head
            AND closure.actor = actor_id AND closure.evidence = evidence_ref
            THEN RETURN to_jsonb(closure); END IF;
        RETURN NULL;
    END IF;
    IF length(btrim(COALESCE(evidence_ref, ''))) NOT BETWEEN 1 AND 500
        OR EXISTS (SELECT 1 FROM craft_queue.recoveries AS r WHERE r.finished_at IS NULL)
        OR EXISTS (SELECT 1 FROM craft_queue.series_members AS s WHERE s.work_id = order_id)
        OR EXISTS (SELECT 1 FROM craft_queue.publications AS p
                   WHERE p.work_id = order_id AND p.state IN ('reserved', 'sending', 'uncertain'))
        THEN RETURN NULL; END IF;

    SELECT m.observation->>'result_commit', latest.sequence INTO result, verified_sequence
    FROM craft_queue.shadow_merge_receipts AS r
    JOIN craft_queue.github_shadow_merges AS m ON m.receipt_id = r.id
    JOIN LATERAL (
        SELECT v.sequence, v.head, v.digest, v.passed
        FROM craft_queue.verifications AS v
        WHERE v.work_id = r.work_id AND v.generation = r.generation
        ORDER BY v.sequence DESC LIMIT 1
    ) AS latest ON latest.passed AND latest.head = r.head AND latest.digest = r.pack_digest
    WHERE r.id = receipt AND r.work_id = order_id AND r.head = expected_head
      AND m.observation->>'head' = expected_head
      AND r.publication_key = craft_queue.current_review_draft(r.work_id, r.generation, r.head, latest.sequence);
    IF NOT FOUND OR COALESCE(result, '') !~ '^([a-f0-9]{40}|[a-f0-9]{64})$' THEN RETURN NULL; END IF;

    INSERT INTO craft_queue.github_work_closures(
        work_id, merge_receipt_id, head, result_commit, actor, evidence, verification_sequence
    )
    VALUES (order_id, receipt, expected_head, result, actor_id, evidence_ref, verified_sequence)
    RETURNING * INTO closure;
    UPDATE craft_queue.work AS w SET state = 'released' WHERE w.id = order_id;
    RETURN to_jsonb(closure);
END;
$$;

COMMIT;
