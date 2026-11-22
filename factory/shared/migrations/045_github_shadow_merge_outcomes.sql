BEGIN;

CREATE TABLE craft_queue.github_shadow_merges (
    receipt_id text PRIMARY KEY REFERENCES craft_queue.shadow_merge_receipts(id),
    verifier text NOT NULL,
    observation jsonb NOT NULL,
    recorded_at timestamptz NOT NULL DEFAULT clock_timestamp()
);

CREATE FUNCTION craft_queue.confirm_github_shadow_merge(
    actor_id text, receipt text, candidate_head text, report_pin text, observed jsonb
) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    binding record;
    previous craft_queue.github_shadow_merges%ROWTYPE;
    merge_time bigint;
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    SELECT r.assessment, p.repository, p.base, p.base_branch, p.result_url INTO binding
    FROM craft_queue.shadow_merge_receipts AS r
    JOIN craft_queue.publications AS p ON p.action_key = r.publication_key AND p.state = 'confirmed'
    JOIN craft_queue.attempts AS a ON a.work_id = r.work_id AND a.generation = r.generation
    WHERE r.id = receipt AND r.head = candidate_head AND r.report_digest = report_pin
        AND r.actor <> actor_id AND a.runner_id <> actor_id AND r.assessment ? 'github'
        AND p.result_url = r.assessment->>'pull_request';
    IF NOT FOUND THEN RETURN NULL; END IF;

    SELECT * INTO previous FROM craft_queue.github_shadow_merges AS m WHERE m.receipt_id = receipt;
    IF observed IS NULL THEN
        RETURN jsonb_build_object('head', candidate_head, 'repository', binding.repository,
            'base', binding.base, 'base_branch', binding.base_branch, 'pull_request', binding.result_url,
            'as_of', binding.assessment->'as_of', 'policy_file_digest', binding.assessment->'github'->'policy_file_digest',
            'recorded', CASE WHEN previous.receipt_id IS NOT NULL THEN to_jsonb(previous) END);
    END IF;
    IF previous.receipt_id IS NOT NULL THEN
        IF previous.verifier = actor_id AND previous.observation = observed THEN RETURN to_jsonb(previous); END IF;
        RETURN NULL;
    END IF;

    IF COALESCE(observed->>'merged_at', '') !~ '^[1-9][0-9]{0,11}$' THEN RETURN NULL; END IF;
    merge_time := (observed->>'merged_at')::bigint;
    IF merge_time <= (binding.assessment->>'as_of')::bigint
        OR merge_time > floor(extract(epoch FROM clock_timestamp()))
        OR observed->>'pull_request' IS DISTINCT FROM binding.result_url
        OR observed->>'head' IS DISTINCT FROM candidate_head
        OR observed->>'pinned_base' IS DISTINCT FROM binding.base
        OR observed->>'destination_branch' IS DISTINCT FROM binding.base_branch
        OR COALESCE(observed->>'actor', '') !~ '^github:user:[1-9][0-9]{0,19}$'
        OR observed->>'result_commit' = binding.base
        OR observed->>'reference' IS DISTINCT FROM ('https://github.com/' || binding.repository
            || '/commit/' || (observed->>'result_commit'))
        OR EXISTS (SELECT 1 FROM craft_queue.recoveries AS r WHERE r.finished_at IS NULL)
        THEN RETURN NULL; END IF;

    INSERT INTO craft_queue.github_shadow_merges (receipt_id, verifier, observation)
    VALUES (receipt, actor_id, observed) RETURNING * INTO previous;
    RETURN to_jsonb(previous);
END;
$$;

CREATE OR REPLACE FUNCTION craft_queue.shadow_merge_receipts_for(order_id text) RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT COALESCE(jsonb_agg(recent.receipt ORDER BY recorded_at DESC, id), '[]'::jsonb)
    FROM (
        SELECT r.recorded_at, r.id, to_jsonb(r) || CASE WHEN m.receipt_id IS NOT NULL
            THEN jsonb_build_object('github_merge', to_jsonb(m)) ELSE '{}'::jsonb END AS receipt
        FROM craft_queue.shadow_merge_receipts AS r
        LEFT JOIN craft_queue.github_shadow_merges AS m ON m.receipt_id = r.id
        WHERE r.work_id = order_id ORDER BY r.recorded_at DESC, r.id LIMIT 20
    ) AS recent;
$$;

REVOKE ALL ON FUNCTION craft_queue.confirm_github_shadow_merge(text, text, text, text, jsonb) FROM PUBLIC;
COMMIT;
