"""Exercise the shared queue against a fresh, disposable PostgreSQL database."""

from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import subprocess


ROOT = Path(__file__).resolve().parent
BASE = "a" * 40
CANDIDATE = "b" * 40


def sql(statement, database=None):
    command = ["psql", "-XAt", "--quiet", "-v", "ON_ERROR_STOP=1"]
    if database is not None:
        command.extend(["--dbname", database])
    try:
        result = subprocess.run(
            command,
            input=statement,
            capture_output=True,
            encoding="utf-8",
            timeout=20,
            check=True,
        )
    except subprocess.CalledProcessError as error:
        error.add_note(error.stderr.strip())
        raise
    return result.stdout.strip()


def admit(order_id, repository, token_limit=25):
    return sql(f"""
        SELECT craft_queue.admit('{order_id}', '{repository}', 'test:{order_id}',
            '{{"schema_version":1,"id":"{order_id}","base":"{BASE}","repository":"C:/private/repo"}}'::jsonb,
            {token_limit});
    """)


def claim(runner, request):
    return sql(f"""
        SELECT work_id || ':' || generation || ':' || (work_order ? 'repository')::text
        FROM craft_queue.claim('{runner}', '{request:08x}-0000-0000-0000-000000000000');
    """)


def submit(runner, order_id, generation, tokens):
    return sql(f"""
        SELECT craft_queue.submit_candidate(
            '{runner}', '{order_id}', {generation}, '{CANDIDATE}', 'artifact:fixture', {tokens});
    """)


def main():
    sql((ROOT / "migrations" / "001_queue.sql").read_text(encoding="utf-8"))
    sql("""
        UPDATE craft_queue.control SET paused = false, token_budget = 100;
        INSERT INTO craft_queue.runner_repositories VALUES
            ('first', 'team/service'), ('second', 'team/service'), ('outside', 'team/other');
    """)
    assert admit("one", "team/service") == "one"
    assert admit("one", "team/service") == "one"
    try:
        admit("one", "team/other")
    except subprocess.CalledProcessError as error:
        assert "identity already has different inputs" in error.stderr
    else:
        raise AssertionError("conflicting admission succeeded")
    assert admit("two", "team/service") == "two"
    assert claim("outside", 1) == ""

    with ThreadPoolExecutor(max_workers=2) as pool:
        results = list(pool.map(lambda runner: claim(runner, 2), ("first", "second")))
    assert sorted(results) == ["", "one:1:false"], results
    winner, loser = ("first", "second") if results[0] else ("second", "first")
    assert claim(winner, 2) == "one:1:false"
    assert claim(loser, 3) == ""
    assert submit(loser, "one", 1, 10) == "f"
    assert sql(f"SELECT craft_queue.renew('{winner}', 'one', 1) IS NOT NULL;") == "t"

    sql("UPDATE craft_queue.attempts SET expires_at = clock_timestamp() - interval '1 second';")
    assert claim(winner, 2) == ""
    assert submit(winner, "one", 1, 10) == "f"
    assert claim(loser, 4) == "one:2:false"
    assert sql(f"SELECT craft_queue.renew('{winner}', 'one', 1) IS NULL;") == "t"
    assert submit(winner, "one", 2, 10) == "f"
    assert submit(loser, "one", 2, 26) == "f"
    assert submit(loser, "one", 2, 10) == "t"
    assert submit(loser, "one", 2, 10) == "t"
    assert submit(loser, "one", 2, 11) == "f"
    assert claim(winner, 5) == ""

    assert sql("SELECT craft_queue.cancel('one', 'Fixture cleanup');") == "t"
    assert claim(winner, 6) == "two:1:false"
    sql("UPDATE craft_queue.control SET paused = true;")
    assert sql(f"SELECT craft_queue.renew('{winner}', 'two', 1) IS NOT NULL;") == "t"
    assert sql("SELECT craft_queue.cancel('two', 'Fixture cleanup');") == "t"
    assert submit(winner, "two", 1, 5) == "f"
    assert claim(loser, 7) == ""
    sql("UPDATE craft_queue.control SET paused = false;")
    assert admit("three", "team/service", 60) == "three"
    assert claim(loser, 8) == ""
    assert sql("""
        SELECT string_agg(work_id || ':' || generation || ':' || outcome, ',' ORDER BY work_id, generation)
        FROM craft_queue.attempts;
    """) == "one:1:expired,one:2:candidate,two:1:cancelled"
    print("Shared PostgreSQL queue passed scope, contention, takeover, replay, pause, and budget checks.")


if __name__ == "__main__":
    main()
