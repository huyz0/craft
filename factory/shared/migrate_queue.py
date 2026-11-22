"""Install or upgrade the Craft queue with psql and a checksum ledger."""

import argparse
from hashlib import sha256
import os
from pathlib import Path
import re
import subprocess
import sys


def migrations(directory):
    entries = []
    for path in sorted(directory.glob("*.sql")):
        match = re.fullmatch(r"([0-9]{3})_[a-z0-9_]+\.sql", path.name)
        if match is None or int(match[1]) != len(entries) + 1:
            raise ValueError(f"Invalid migration order or name at {path.name}")
        text = path.read_bytes().decode("utf-8").replace("\r\n", "\n")
        # The runner commits the schema change and its ledger entry together.
        body = text.strip()
        if not body.startswith("BEGIN;") or not body.endswith("COMMIT;"):
            raise ValueError(f"{path.name} must start with BEGIN and end with COMMIT")
        entries.append((int(match[1]), path.name, sha256(text.encode()).hexdigest(), body[6:-7]))
    if not entries:
        raise ValueError("No queue migrations found")
    return entries


def service_functions(path):
    signatures = path.read_text(encoding="utf-8").splitlines()
    pattern = r"craft_queue\.[a-z_]+\((?:[a-z]+(?:\[\])?(?:,[a-z]+(?:\[\])?)*)?\)"
    if not signatures or len(set(signatures)) != len(signatures) or not all(
        re.fullmatch(pattern, signature) for signature in signatures
    ):
        raise ValueError("Invalid queue service function manifest")
    return ",\n    ".join(signatures)


def script(entries, role, baseline, functions):
    expected = ",\n".join(f"({number}, '{name}', '{digest}')" for number, name, digest, _ in entries)
    steps = [f"""
SET search_path = pg_catalog, pg_temp;
DO $bootstrap$
BEGIN
    PERFORM pg_advisory_lock(1266275406, 195980241);
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = '{role}' AND rolcanlogin AND NOT rolsuper)
       OR current_user = '{role}' THEN
        RAISE EXCEPTION 'Provide a separate, existing, non-superuser service login';
    END IF;
END;
$bootstrap$;
CREATE TEMP TABLE expected_queue_migrations (number integer PRIMARY KEY, filename text, digest text);
INSERT INTO expected_queue_migrations VALUES {expected};
BEGIN;
CREATE SCHEMA IF NOT EXISTS craft_migration;
REVOKE ALL ON SCHEMA craft_migration FROM PUBLIC;
CREATE TABLE IF NOT EXISTS craft_migration.queue_history (
    number integer PRIMARY KEY,
    filename text NOT NULL,
    digest text NOT NULL,
    external_baseline boolean NOT NULL DEFAULT false,
    recorded_at timestamptz NOT NULL DEFAULT clock_timestamp()
);
REVOKE ALL ON TABLE craft_migration.queue_history FROM PUBLIC;
DO $bootstrap$
DECLARE
    current_version integer;
    schema_exists boolean := to_regnamespace('craft_queue') IS NOT NULL;
    baseline integer := {baseline};
BEGIN
    SELECT COALESCE(max(number), 0) INTO current_version FROM craft_migration.queue_history;
    IF (SELECT count(*) FROM craft_migration.queue_history) <> current_version OR EXISTS (
        SELECT 1 FROM craft_migration.queue_history AS recorded
        LEFT JOIN expected_queue_migrations AS expected USING (number)
        WHERE (recorded.filename, recorded.digest) IS DISTINCT FROM (expected.filename, expected.digest)
    ) THEN
        RAISE EXCEPTION 'Queue migration history differs from the installed migration files';
    END IF;
    IF schema_exists AND current_version = 0 THEN
        IF baseline = 0 THEN RAISE EXCEPTION 'Existing queue needs an explicit verified migration baseline'; END IF;
        INSERT INTO craft_migration.queue_history (number, filename, digest, external_baseline)
        SELECT number, filename, digest, true FROM expected_queue_migrations WHERE number <= baseline;
    ELSIF current_version > 0 AND NOT schema_exists THEN
        RAISE EXCEPTION 'Queue migration history exists but the queue schema is missing';
    ELSIF baseline <> 0 AND (SELECT count(*) FROM craft_migration.queue_history WHERE external_baseline) <> baseline THEN
        RAISE EXCEPTION 'The requested baseline does not match the migration ledger';
    END IF;
END;
$bootstrap$;
COMMIT;
"""]
    for number, name, digest, body in entries:
        steps.append(f"""
SELECT NOT EXISTS (SELECT 1 FROM craft_migration.queue_history WHERE number = {number}) AS pending \\gset
\\if :pending
\\echo Applying {name}
BEGIN;
{body}
INSERT INTO craft_migration.queue_history (number, filename, digest) VALUES ({number}, '{name}', '{digest}');
COMMIT;
\\endif
""")
    steps.append(f"""
BEGIN;
GRANT USAGE ON SCHEMA craft_queue TO "{role}";
REVOKE ALL ON ALL FUNCTIONS IN SCHEMA craft_queue FROM PUBLIC, "{role}";
GRANT EXECUTE ON FUNCTION
    {functions}
TO "{role}";
COMMIT;
\\echo Queue migrations and service grants ready through {entries[-1][0]:03d}
""")
    return "\n".join(steps)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--service-role", required=True, help="existing restricted queue service login")
    parser.add_argument("--baseline-through", type=int, default=0,
                        help="last migration independently verified on an existing unmanaged queue")
    options = parser.parse_args()
    if not re.fullmatch(r"[a-z_][a-z0-9_]{0,62}", options.service_role):
        parser.error("--service-role must be a lowercase PostgreSQL role name")
    try:
        root = Path(__file__).resolve().parent
        entries = migrations(root / "migrations")
        if not 0 <= options.baseline_through <= entries[-1][0]:
            parser.error("--baseline-through must refer to a packaged migration")
        result = subprocess.run(["psql", "-X", "--quiet", "--no-password", "--set", "ON_ERROR_STOP=1", "-f", "-"],
                                input=script(entries, options.service_role, options.baseline_through,
                                             service_functions(root / "service-functions.txt")).encode(),
                                env=os.environ | {"PGCLIENTENCODING": "UTF8"})
        return result.returncode
    except (OSError, ValueError) as error:
        print(f"Queue migration failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
