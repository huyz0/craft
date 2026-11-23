BEGIN;

CREATE OR REPLACE FUNCTION craft_queue.candidate_pack(order_id text) RETURNS jsonb
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT jsonb_build_object(
        'work_id', w.id, 'repository', w.repository, 'generation', w.generation,
        'commit', w.candidate, 'digest', p.digest,
        'work_order', w.payload - 'repository',
        'pack_base64', replace(encode(p.payload, 'base64'), E'\n', '')
    )
    FROM craft_queue.work AS w JOIN craft_queue.candidate_packs AS p
      ON p.work_id = w.id AND p.generation = w.generation
    WHERE w.id = order_id AND w.state = 'candidate';
$$;

COMMIT;
