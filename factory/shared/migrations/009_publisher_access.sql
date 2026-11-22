BEGIN;

CREATE FUNCTION craft_queue.prepare_draft(actor text, key text) RETURNS jsonb
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT to_jsonb(p) || jsonb_build_object(
        'work_order', w.payload - 'repository',
        'pack_base64', replace(encode(pack.payload, 'base64'), E'\n', '')
    )
    FROM craft_queue.publications AS p
    JOIN craft_queue.draft_policies AS policy ON policy.repository = p.repository
    JOIN craft_queue.work AS w ON w.id = p.work_id
    JOIN craft_queue.candidate_packs AS pack ON pack.work_id = p.work_id AND pack.generation = p.generation
    CROSS JOIN craft_queue.control AS c
    JOIN LATERAL (
        SELECT v.sequence, v.passed FROM craft_queue.verifications AS v
         WHERE v.work_id = p.work_id AND v.generation = p.generation
         ORDER BY v.sequence DESC LIMIT 1
    ) AS latest ON true
    WHERE p.action_key = key AND p.state = 'reserved' AND NOT c.paused
      AND p.publisher_id = actor AND policy.publisher_id = actor AND policy.enabled
      AND policy.base_branch = p.base_branch AND latest.passed AND latest.sequence = p.verification_sequence
      AND w.state = 'candidate' AND w.generation = p.generation AND w.candidate = p.head
      AND w.candidate_evidence = p.pack_digest AND pack.digest = p.pack_digest;
$$;

REVOKE ALL ON FUNCTION craft_queue.prepare_draft(text, text) FROM PUBLIC;
COMMIT;
