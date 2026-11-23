use serde_json::{Value, json};
use sqlx::PgPool;

use crate::Error;

const SERVICE_FUNCTIONS: &str = include_str!("../service-functions.txt");
const MIGRATION_MANIFEST: &str =
    include_str!(concat!(env!("OUT_DIR"), "/queue-migration-manifest.txt"));

pub async fn check_database(database: &PgPool) -> Result<Value, Error> {
    let signatures: Vec<_> = SERVICE_FUNCTIONS.lines().collect();
    let (functions_present, functions_granted, schema_usage, broad_access, excess_execute) =
        sqlx::query_as::<_, (bool, bool, bool, bool, bool)>(
            "WITH functions AS (
                SELECT to_regprocedure(signature)::oid AS oid FROM unnest($1::text[]) AS signature
            ), reachable_roles AS MATERIALIZED (
                SELECT oid, rolsuper, rolcreaterole, rolcreatedb, rolreplication, rolbypassrls
                FROM pg_roles
                WHERE rolname IN (session_user, current_user)
                   OR pg_has_role(session_user, oid, CASE
                       WHEN current_setting('server_version_num')::integer >= 160000 THEN 'SET'
                       ELSE 'MEMBER' END)
            )
            SELECT
                (SELECT bool_and(oid IS NOT NULL) FROM functions),
                (SELECT bool_and(COALESCE(has_function_privilege(oid, 'EXECUTE'), false)) FROM functions),
                COALESCE(has_schema_privilege(to_regnamespace('craft_queue'), 'USAGE'), false),
                EXISTS (
                    SELECT 1 FROM reachable_roles AS access_role
                    WHERE access_role.rolsuper OR access_role.rolcreaterole OR access_role.rolcreatedb
                       OR access_role.rolreplication OR access_role.rolbypassrls
                       OR has_schema_privilege(access_role.oid, to_regnamespace('craft_queue'), 'CREATE')
                       OR has_schema_privilege(access_role.oid, to_regnamespace('craft_migration'), 'CREATE')
                       OR EXISTS (
                           SELECT 1 FROM pg_roles AS privileged_role
                           WHERE privileged_role.rolname IN (
                               'pg_read_server_files', 'pg_write_server_files', 'pg_execute_server_program'
                           ) AND pg_has_role(access_role.oid, privileged_role.oid, 'USAGE')
                       )
                       OR EXISTS (
                           SELECT 1 FROM pg_class AS relation
                           JOIN pg_namespace AS namespace ON namespace.oid = relation.relnamespace
                           WHERE namespace.nspname IN ('craft_queue', 'craft_migration')
                             AND relation.relkind IN ('r', 'p')
                             AND (has_table_privilege(access_role.oid, relation.oid,
                                  'SELECT,INSERT,UPDATE,DELETE,TRUNCATE,REFERENCES,TRIGGER')
                                  OR has_any_column_privilege(access_role.oid, relation.oid,
                                      'SELECT,INSERT,UPDATE,REFERENCES'))
                       )
                ),
                EXISTS (
                    SELECT 1 FROM pg_proc AS routine JOIN pg_namespace AS namespace ON namespace.oid = routine.pronamespace
                    CROSS JOIN reachable_roles AS access_role
                    WHERE namespace.nspname = 'craft_queue' AND routine.prokind = 'f'
                    AND (has_function_privilege(access_role.oid, routine.oid, 'EXECUTE WITH GRANT OPTION')
                         OR (has_function_privilege(access_role.oid, routine.oid, 'EXECUTE') AND NOT EXISTS (
                             SELECT 1 FROM functions WHERE functions.oid = routine.oid)))
                )",
        )
        .bind(&signatures)
        .fetch_one(database)
        .await
        .map_err(|_| "could not inspect shared PostgreSQL queue permissions")?;
    if !functions_present {
        return Err("the shared queue schema is incomplete; apply the packaged migrations".into());
    }
    if !schema_usage || !functions_granted {
        return Err(
            "the service login is missing queue function grants; rerun the migration helper".into(),
        );
    }
    if broad_access {
        return Err("the service login has administrative or direct queue table access".into());
    }
    if excess_execute {
        return Err(
            "the service login has excess queue function grants; rerun the migration helper".into(),
        );
    }
    let matches = sqlx::query_scalar::<_, bool>("SELECT craft_queue.matches_migration_history($1)")
        .bind(MIGRATION_MANIFEST.lines().collect::<Vec<_>>())
        .fetch_one(database)
        .await
        .map_err(|_| "could not inspect shared PostgreSQL queue migration history")?;
    if !matches {
        return Err("the shared queue migration history differs from this build; verify the database and packaged migrations".into());
    }
    let summary = sqlx::query_scalar::<_, Value>("SELECT craft_queue.operation_summary()")
        .fetch_one(database)
        .await
        .map_err(|_| "could not read shared queue status using the service login")?;
    let paused = summary["paused"]
        .as_bool()
        .ok_or("invalid shared queue status")?;
    Ok(json!({"schema": "craft_queue", "paused": paused}))
}
