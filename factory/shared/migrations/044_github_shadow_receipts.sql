BEGIN;

ALTER FUNCTION craft_queue.record_shadow_merge(text, text, text, bigint, text, text, bigint, text, jsonb)
    RENAME TO record_shadow_merge_receipt;

CREATE FUNCTION craft_queue.record_shadow_merge(
    actor_id text, receipt_id text, order_id text, candidate_generation bigint, candidate_head text,
    candidate_digest text, review_sequence bigint, digest text, assessment_data jsonb
) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    confirmed_url text;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    SELECT p.result_url INTO confirmed_url FROM craft_queue.publications AS p
    WHERE p.work_id = order_id AND p.generation = candidate_generation AND p.head = candidate_head
        AND p.pack_digest = candidate_digest AND p.state = 'confirmed';
    IF confirmed_url IS NULL THEN RETURN NULL; END IF;

    IF confirmed_url LIKE 'https://github.com/%' THEN
        IF jsonb_typeof(assessment_data->'github') IS DISTINCT FROM 'object'
            OR assessment_data->>'pull_request' IS DISTINCT FROM confirmed_url
            OR assessment_data->'eligible' IS DISTINCT FROM assessment_data->'native_verified'
            THEN RETURN NULL;
        END IF;
    ELSIF assessment_data ? 'github' THEN RETURN NULL;
    END IF;

    RETURN craft_queue.record_shadow_merge_receipt(actor_id, receipt_id, order_id, candidate_generation,
        candidate_head, candidate_digest, review_sequence, digest, assessment_data);
END;
$$;

REVOKE ALL ON FUNCTION craft_queue.record_shadow_merge(text, text, text, bigint, text, text, bigint, text, jsonb)
    FROM PUBLIC;
COMMIT;
