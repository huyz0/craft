BEGIN;

CREATE TABLE craft_queue.runner_claim_polls (
    runner_id text NOT NULL,
    adapter text NOT NULL CHECK (adapter IN ('codex', 'claude')),
    observed_at timestamptz NOT NULL,
    available_repositories text[],
    context_capable boolean NOT NULL,
    profile_capable boolean NOT NULL,
    PRIMARY KEY (runner_id, adapter)
);

ALTER FUNCTION craft_queue.claim(text, uuid, boolean, boolean, text[], text) RENAME TO claim_before_poll_receipts;

CREATE FUNCTION craft_queue.claim(
    runner text, request uuid, context_capable boolean, profile_capable boolean, available_repositories text[], selected_adapter text
)
RETURNS TABLE (work_id text, repository text, generation bigint, expires_at timestamptz, work_order jsonb)
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
BEGIN
    IF COALESCE(selected_adapter, '') NOT IN ('codex', 'claude') THEN RETURN; END IF;
    RETURN QUERY SELECT * FROM craft_queue.claim_before_poll_receipts(
        runner, request, context_capable, profile_capable, available_repositories, selected_adapter);
    INSERT INTO craft_queue.runner_claim_polls AS p
        (runner_id, adapter, observed_at, available_repositories, context_capable, profile_capable)
    SELECT runner, selected_adapter, clock_timestamp(),
        CASE WHEN $5 IS NOT NULL THEN COALESCE(
            array_agg(e.repository ORDER BY e.repository) FILTER (WHERE e.repository = ANY($5)),
            ARRAY[]::text[])
        END,
        COALESCE($3, false), COALESCE($4, false)
    FROM craft_queue.runner_repositories AS e
    WHERE e.runner_id = runner
    HAVING count(*) > 0
    ON CONFLICT (runner_id, adapter) DO UPDATE
    SET observed_at = EXCLUDED.observed_at,
        available_repositories = EXCLUDED.available_repositories,
        context_capable = EXCLUDED.context_capable,
        profile_capable = EXCLUDED.profile_capable;
    RETURN;
END;
$$;

CREATE FUNCTION craft_queue.clear_runner_claim_polls_after_restore() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog AS $$
BEGIN
    DELETE FROM craft_queue.runner_claim_polls;
    RETURN NEW;
END;
$$;

CREATE TRIGGER clear_runner_claim_polls_after_restore AFTER INSERT ON craft_queue.recoveries
FOR EACH ROW EXECUTE FUNCTION craft_queue.clear_runner_claim_polls_after_restore();

ALTER FUNCTION craft_queue.dispatch_status(text) RENAME TO dispatch_status_before_poll_receipts;

CREATE FUNCTION craft_queue.dispatch_status(order_id text) RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    WITH snapshot AS MATERIALIZED (
        SELECT craft_queue.dispatch_status_before_poll_receipts(order_id) AS info
    ), matching AS MATERIALIZED (
        SELECT e.runner_id, p.observed_at,
            greatest(floor(extract(epoch FROM statement_timestamp() - p.observed_at)), 0) AS age_seconds,
            e.repository = ANY(p.available_repositories) AS checkout_reported,
            p.context_capable, p.profile_capable,
            COALESCE(latest.mode, 'active') AS mode,
            craft_queue.standing_runner_allowed(order_id, e.runner_id, statement_timestamp()) AS standing_route
        FROM snapshot
        JOIN craft_queue.runner_repositories AS e ON e.repository = snapshot.info->>'repository'
        LEFT JOIN craft_queue.runner_claim_polls AS p
          ON p.runner_id = e.runner_id AND p.adapter = snapshot.info->>'required_adapter'
        LEFT JOIN LATERAL (
            SELECT c.mode FROM craft_queue.runner_controls AS c WHERE c.runner_id = e.runner_id
            ORDER BY c.revision DESC LIMIT 1
        ) AS latest ON true
    )
    SELECT snapshot.info || jsonb_build_object(
        'claim_poll_reports', COALESCE((
            SELECT jsonb_agg(to_jsonb(item) ORDER BY (mode = 'active') DESC, observed_at DESC NULLS LAST, runner_id)
            FROM (
                SELECT * FROM matching
                ORDER BY (mode = 'active') DESC, observed_at DESC NULLS LAST, runner_id LIMIT 20
            ) AS item
        ), '[]'::jsonb),
        'claim_poll_reports_truncated', (SELECT count(*) FROM matching) > 20
    ) FROM snapshot;
$$;

REVOKE ALL ON FUNCTION craft_queue.claim(text, uuid, boolean, boolean, text[], text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.dispatch_status(text) FROM PUBLIC;
REVOKE ALL ON FUNCTION craft_queue.clear_runner_claim_polls_after_restore() FROM PUBLIC;

COMMIT;
