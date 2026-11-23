BEGIN;

CREATE FUNCTION craft_queue.work_has_external_evidence(
    order_id text, generation_pin bigint, head_pin text, pack_pin text
) RETURNS boolean LANGUAGE sql STABLE SET search_path = pg_catalog AS $$
    SELECT craft_queue.github_enqueue_blocks_work(order_id)
        OR EXISTS (
            SELECT 1 FROM craft_queue.publications AS p
            WHERE p.work_id = order_id AND p.state IN ('reserved', 'sending', 'uncertain')
        )
        OR craft_queue.current_bitbucket_merge(order_id, generation_pin, head_pin, pack_pin)
        OR EXISTS (
            SELECT 1 FROM craft_queue.shadow_merge_receipts AS r
            JOIN craft_queue.github_shadow_merges AS m ON m.receipt_id = r.id
            JOIN craft_queue.publications AS p ON p.action_key = r.publication_key AND p.state = 'confirmed'
            WHERE r.work_id = order_id AND r.generation = generation_pin AND r.head = head_pin
              AND r.pack_digest = pack_pin AND m.observation->>'head' = head_pin
              AND p.work_id = r.work_id AND p.generation = r.generation
              AND p.head = r.head AND p.pack_digest = r.pack_digest
              AND m.observation->>'pull_request' = p.result_url
        );
$$;

CREATE OR REPLACE FUNCTION craft_queue.fence_standing_work(policy_id text, only_issue text, reason text)
RETURNS void
LANGUAGE sql SET search_path = pg_catalog AS $$
    WITH blocked AS (
        UPDATE craft_queue.standing_work AS binding SET blocked_at = clock_timestamp()
        WHERE binding.rule_id = policy_id AND (only_issue IS NULL OR binding.issue_key = only_issue)
          AND binding.blocked_at IS NULL
        RETURNING binding.work_id
    ), cancelled AS (
        UPDATE craft_queue.work AS w SET state = 'cancelled', cancellation_reason = reason
        FROM blocked WHERE w.id = blocked.work_id AND w.state IN ('queued', 'leased', 'candidate')
          AND NOT craft_queue.work_has_external_evidence(w.id, w.generation, w.candidate, w.candidate_evidence)
        RETURNING w.id
    )
    UPDATE craft_queue.attempts AS a SET outcome = 'cancelled'
    FROM cancelled WHERE a.work_id = cancelled.id AND a.outcome = 'leased';
$$;

CREATE OR REPLACE FUNCTION craft_queue.revoke_team_skill(skill_location text, skill_revision text, evidence_ref text)
RETURNS boolean
LANGUAGE plpgsql SET search_path = pg_catalog AS $$
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    UPDATE craft_queue.team_skill_releases AS s
    SET revoked_at = clock_timestamp(), revocation_reference = evidence_ref
    WHERE s.location = skill_location AND s.revision = skill_revision AND s.revoked_at IS NULL;
    IF NOT FOUND THEN
        RETURN EXISTS (
            SELECT 1 FROM craft_queue.team_skill_releases AS s WHERE s.location = skill_location
              AND s.revision = skill_revision AND s.revocation_reference = evidence_ref
        );
    END IF;
    WITH cancelled AS (
        UPDATE craft_queue.work AS w
        SET state = 'cancelled', cancellation_reason = 'Approved team skill release revoked'
        WHERE w.payload #>> '{skill,location}' = skill_location
          AND w.payload #>> '{skill,revision}' = skill_revision
          AND w.state IN ('queued', 'leased', 'candidate')
          AND NOT craft_queue.work_has_external_evidence(w.id, w.generation, w.candidate, w.candidate_evidence)
        RETURNING w.id
    )
    UPDATE craft_queue.attempts AS a SET outcome = 'cancelled'
    FROM cancelled WHERE a.work_id = cancelled.id AND a.outcome = 'leased';
    RETURN true;
END;
$$;

CREATE OR REPLACE FUNCTION craft_queue.retire_project_context(
    repo text, context_location text, context_revision text, evidence_ref text
) RETURNS boolean
LANGUAGE plpgsql SET search_path = pg_catalog AS $$
BEGIN
    PERFORM 1 FROM craft_queue.control FOR UPDATE;
    UPDATE craft_queue.project_context_releases AS release
    SET retired_at = clock_timestamp(), retirement_reference = evidence_ref
    WHERE release.repository = repo AND release.location = context_location
      AND release.revision = context_revision AND release.retired_at IS NULL;
    IF NOT FOUND THEN
        RETURN EXISTS (
            SELECT 1 FROM craft_queue.project_context_releases AS release WHERE release.repository = repo
              AND release.location = context_location AND release.revision = context_revision
              AND release.retirement_reference = evidence_ref
        );
    END IF;
    WITH cancelled AS (
        UPDATE craft_queue.work AS w
        SET state = 'cancelled', cancellation_reason = 'Approved project context retired'
        WHERE w.repository = repo AND w.state IN ('queued', 'leased', 'candidate')
          AND EXISTS (
              SELECT 1 FROM craft_queue.project_context_pins(w.payload) AS pin
              WHERE pin.location = context_location AND pin.revision = context_revision
          )
          AND NOT craft_queue.work_has_external_evidence(w.id, w.generation, w.candidate, w.candidate_evidence)
        RETURNING w.id
    )
    UPDATE craft_queue.attempts AS a SET outcome = 'cancelled'
    FROM cancelled WHERE a.work_id = cancelled.id AND a.outcome = 'leased';
    RETURN true;
END;
$$;

REVOKE ALL ON FUNCTION craft_queue.work_has_external_evidence(text,bigint,text,text) FROM PUBLIC;

COMMIT;
