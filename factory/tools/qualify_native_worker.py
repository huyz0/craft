"""Run an installed Codex worker with a loopback model and disposable Git checkouts."""

import argparse
import json
import os
from pathlib import Path
import subprocess
from tempfile import TemporaryDirectory

from qualify_native_coordinator import MODEL, item_stream, native_codex_fixture, response_stream


def git(repository, *arguments):
    env = {name: value for name, value in os.environ.items() if not name.upper().startswith("GIT_")}
    env.update(GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull)
    return subprocess.check_output(["git", "-C", repository, *arguments], env=env, text=True, timeout=15).strip()


def edit_response(requests, path, greeting):
    if len(requests) == 2:
        return response_stream({"outcome": "candidate", "summary": "The fixture changed one greeting."}, "resp_claim")
    if len(requests) != 1:
        raise RuntimeError("the fixture expected one tool call followed by one final claim")
    tool = next((tool for tool in requests[0]["tools"] if tool.get("name") == "exec_command"), {})
    if tool.get("type") != "function" or "cmd" not in tool.get("parameters", {}).get("properties", {}):
        raise RuntimeError("the installed worker did not offer exec_command with a cmd argument")
    if os.name == "nt":
        command = f"Set-Content -LiteralPath '{path}' -Value '{greeting}'"
    else:
        command = f"printf '%s\\n' '{greeting}' > '{path}'"
    call = {"type": "function_call", "name": "exec_command", "id": "fc_fixture",
            "call_id": "call_fixture", "arguments": json.dumps({"cmd": command}), "status": "completed"}
    return item_stream(call, call | {"arguments": "", "status": "in_progress"}, "resp_edit")


def prepare_order(root):
    repository = root / "source"
    repository.mkdir()
    git(repository, "init", "--quiet", "--template=")
    git(repository, "config", "user.name", "Craft fixture")
    git(repository, "config", "user.email", "craft-fixture@example.invalid")
    git(repository, "config", "core.autocrlf", "false")
    for filename, contents in (
        ("src/greeting.txt", "original greeting\n"),
        ("docs/greeting.txt", "original greeting\n"),
        ("skill/SKILL.md", "# Greeting fixture\nChange the approved greeting and report the result.\n"),
    ):
        file = repository / filename
        file.parent.mkdir(exist_ok=True)
        file.write_bytes(contents.encode("utf-8"))
    git(repository, "add", ".")
    git(repository, "commit", "--quiet", "-m", "Fixture base")
    base = git(repository, "rev-parse", "HEAD")
    return {
        "schema_version": 1, "id": "native-worker", "issue": "FIXTURE-1", "owner": "Fixture owner",
        "repository": str(repository), "base": base, "intent": "Change the greeting in src/greeting.txt.",
        "acceptance": ["The approved greeting reads changed greeting."], "scope": ["src"],
        "context": [{"location": "fixture:approved-greeting", "revision": base}],
        "skill": {"location": "repo:skill/SKILL.md", "revision": base},
        "execution_profile": {"id": "fixture-worker", "model": MODEL, "reasoning_effort": "medium",
                              "timeout_seconds": 90, "max_attempts": 1, "approval_reference": "fixture:worker"},
    }


def run_native_worker(codex, root, order, path, launch, *, greeting="changed greeting", repair_context=()):
    temp = root / "worker-temp"
    temp.mkdir()
    with native_codex_fixture(
        codex, root, lambda requests: edit_response(requests, path, greeting)
    ) as (shim, home, requests):
        run = launch(["--codex", shim, "--codex-home", home, "--timeout-seconds", "90"],
                     {"TEMP": str(temp), "TMP": str(temp), "TMPDIR": str(temp)})
        if len(requests) != 2:
            stderr = (root / "native-stderr.txt").read_text(encoding="utf-8", errors="replace")
            raise RuntimeError(f"Expected two model requests, got {len(requests)}: {run.stderr}\n{stderr[-3000:]}")
        if run.returncode:
            tool_outputs = [item.get("output") for item in requests[1].get("input", [])
                            if isinstance(item, dict) and item.get("type") == "function_call_output"]
            run.stderr += "\nFixture tool output: " + json.dumps(tool_outputs)[-2500:]
        assert not (home / "auth.json").exists()
    assert requests[0]["model"] == MODEL and requests[0]["reasoning"]["effort"] == "medium"
    assert requests[0]["text"]["format"]["schema"]["properties"]["outcome"]["enum"] == ["candidate", "blocked"]
    inputs = json.dumps(requests[0]["input"])
    assert all(text in inputs for text in (order["intent"], *repair_context))
    return run


def qualify(launcher, codex, root, *, path, allowed):
    order = prepare_order(root)
    repository = Path(order["repository"])
    base = order["base"]
    order_path = root / "order.json"
    order_path.write_text(json.dumps(order), encoding="utf-8")
    command = [launcher, "--state", root / "queue.db"]
    subprocess.run(command + ["delegate", order_path], stdout=subprocess.DEVNULL, check=True, timeout=15)
    destination = root / "attempt"

    def launch(options, temp):
        return subprocess.run(command + ["run", order["id"], "--destination", destination, *options],
                              capture_output=True, text=True, timeout=100, env=os.environ | temp)

    run = run_native_worker(codex, root, order, path, launch)
    assert git(repository, "rev-parse", "HEAD") == base and not git(repository, "status", "--porcelain")
    status = json.loads(subprocess.check_output(command + ["status", order["id"]], timeout=15))
    observation = next(event for event in status["events"] if event["kind"] == "runner_observed")
    usage = json.loads(observation["note"])
    assert (usage["input_tokens"], usage["cached_input_tokens"], usage["output_tokens"]) == (200, 40, 100), usage
    if allowed:
        assert run.returncode == 0, run.stderr
        candidate = status["candidate"]
        assert git(repository, "rev-parse", f"refs/craft-factory/candidates/{order['id']}/{candidate}") == candidate
        assert git(repository, "diff", "--name-only", base, candidate) == path
        assert git(repository, "show", f"{candidate}:{path}") == "changed greeting"
    else:
        assert run.returncode != 0 and "scope" in run.stderr.lower(), run.stderr
        assert (destination / "checkout" / path).read_text().strip() == "changed greeting"
        assert status["state"] == "delegated" and status["candidate"] is None
        assert not git(repository, "for-each-ref", "refs/craft-factory/candidates")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("launcher", type=Path)
    parser.add_argument("codex", type=Path)
    args = parser.parse_args()
    for path, allowed in (("src/greeting.txt", True), ("docs/greeting.txt", False)):
        with TemporaryDirectory(prefix="craft native worker ") as scratch:
            qualify(args.launcher.resolve(strict=True), args.codex.resolve(strict=True), Path(scratch).resolve(),
                    path=path, allowed=allowed)
    print("Installed Codex passed loopback editing, exact candidate import, scope rejection, and usage checks.")


if __name__ == "__main__":
    main()
