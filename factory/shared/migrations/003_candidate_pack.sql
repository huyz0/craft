BEGIN;

CREATE TABLE craft_queue.candidate_packs (
    work_id text NOT NULL,
    generation bigint NOT NULL,
    digest text NOT NULL,
    payload bytea NOT NULL CHECK (octet_length(payload) BETWEEN 1 AND 524288),
    PRIMARY KEY (work_id, generation),
    FOREIGN KEY (work_id, generation) REFERENCES craft_queue.attempts(work_id, generation)
);

CREATE FUNCTION craft_queue.submit_candidate_pack(
    runner text, lease_work_id text, lease_generation bigint, commit_id text, pack bytea, tokens bigint
) RETURNS text
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    reference text;
BEGIN
    IF pack IS NULL OR octet_length(pack) NOT BETWEEN 1 AND 524288
       OR substring(pack FROM 1 FOR 4) <> decode('5041434b', 'hex') THEN
        RAISE EXCEPTION 'invalid candidate pack';
    END IF;
    reference := 'sha256:' || encode(sha256(pack), 'hex');
    IF NOT craft_queue.submit_candidate(
        runner, lease_work_id, lease_generation, commit_id, reference, tokens
    ) THEN
        RETURN NULL;
    END IF;
    INSERT INTO craft_queue.candidate_packs (work_id, generation, digest, payload)
    VALUES (lease_work_id, lease_generation, reference, pack)
    ON CONFLICT (work_id, generation) DO NOTHING;
    RETURN reference;
END;
$$;

CREATE FUNCTION craft_queue.candidate_pack(order_id text) RETURNS jsonb
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT jsonb_build_object('work_id', w.id, 'repository', w.repository,
                              'generation', w.generation, 'commit', w.candidate,
                              'digest', p.digest, 'pack_base64', encode(p.payload, 'base64'))
    FROM craft_queue.work AS w JOIN craft_queue.candidate_packs AS p
      ON p.work_id = w.id AND p.generation = w.generation
    WHERE w.id = order_id AND w.state = 'candidate';
$$;

REVOKE ALL ON ALL FUNCTIONS IN SCHEMA craft_queue FROM PUBLIC;
COMMIT;
