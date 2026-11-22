BEGIN;

CREATE FUNCTION craft_queue.runner_context_allowed(
    work_row craft_queue.work, lease craft_queue.attempts, runner text, requested_generation bigint
) RETURNS boolean
LANGUAGE sql STABLE SET search_path = pg_catalog AS $$
    SELECT (work_row).state = 'leased' AND (work_row).generation = requested_generation
      AND (lease).work_id = (work_row).id AND (lease).generation = requested_generation
      AND (lease).runner_id = runner AND (lease).outcome = 'leased' AND (lease).expires_at > statement_timestamp()
      AND EXISTS (SELECT 1 FROM craft_queue.runner_repositories AS enrollment
                  WHERE enrollment.repository = (work_row).repository AND enrollment.runner_id = runner)
      AND ((work_row).payload->>'agent_adapter' IS DISTINCT FROM 'claude' OR (lease).claude_approval_id IS NOT NULL)
      AND craft_queue.team_skill_allowed((work_row).repository, (work_row).payload)
      AND craft_queue.project_context_allowed((work_row).repository, (work_row).payload)
      AND craft_queue.series_blocker((work_row).id) IS NULL
      AND craft_queue.standing_runner_allowed((work_row).id, runner, statement_timestamp());
$$;

REVOKE ALL ON FUNCTION craft_queue.runner_context_allowed(
    craft_queue.work, craft_queue.attempts, text, bigint
) FROM PUBLIC;

CREATE OR REPLACE FUNCTION craft_queue.repair_context(runner text, order_id text, current_generation bigint)
RETURNS jsonb
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT jsonb_build_object(
        'head', r.head, 'native_url', r.native_url, 'note', r.note, 'evidence', r.evidence,
        'digest', p.digest, 'pack_base64', replace(encode(p.payload, 'base64'), E'\n', '')
    )
    FROM craft_queue.work AS w
    JOIN craft_queue.repair_requests AS r ON r.work_id = w.id AND r.from_generation = w.repair_from_generation
    JOIN craft_queue.candidate_packs AS p ON p.work_id = r.work_id AND p.generation = r.from_generation
    JOIN craft_queue.attempts AS a ON a.work_id = w.id AND a.generation = w.generation
    WHERE w.id = order_id AND craft_queue.runner_context_allowed(w, a, runner, current_generation);
$$;

CREATE OR REPLACE FUNCTION craft_queue.team_skill_for_lease(runner text, order_id text, lease_generation bigint)
RETURNS jsonb
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT jsonb_build_object(
        'location', s.location, 'revision', s.revision, 'markdown', s.markdown,
        'owner', s.owner_name, 'approval_reference', s.approval_reference
    )
    FROM craft_queue.work AS w
    JOIN craft_queue.attempts AS a ON a.work_id = w.id AND a.generation = w.generation
    JOIN craft_queue.team_skill_releases AS s
      ON s.location = w.payload #>> '{skill,location}' AND s.revision = w.payload #>> '{skill,revision}'
    WHERE w.id = order_id AND craft_queue.runner_context_allowed(w, a, runner, lease_generation)
      AND s.revoked_at IS NULL AND w.repository = ANY(s.repositories);
$$;

CREATE OR REPLACE FUNCTION craft_queue.project_context_for_lease(runner text, order_id text, lease_generation bigint)
RETURNS jsonb
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT (
        SELECT COALESCE(jsonb_agg(jsonb_build_object(
            'location', c.location, 'revision', c.revision, 'excerpt', c.excerpt, 'kind', c.kind,
            'source_reference', c.source_reference, 'source_revision', c.source_revision,
            'approval_reference', c.approval_reference
        ) ORDER BY c.location), '[]'::jsonb)
        FROM craft_queue.project_context_pins(w.payload) AS pin
        JOIN craft_queue.project_context_releases AS c
          ON c.repository = w.repository AND c.location = pin.location AND c.revision = pin.revision
         AND c.retired_at IS NULL
    )
    FROM craft_queue.work AS w
    JOIN craft_queue.attempts AS a ON a.work_id = w.id AND a.generation = w.generation
    WHERE w.id = order_id AND craft_queue.runner_context_allowed(w, a, runner, lease_generation);
$$;

COMMIT;
