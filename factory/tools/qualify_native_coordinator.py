"""Check the extracted launcher and an installed Codex CLI against a loopback model fixture."""

import argparse
from contextlib import contextmanager
from hashlib import sha256
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import subprocess
import sys
from tempfile import TemporaryDirectory
from threading import Thread


SEED = Path(__file__).resolve().parents[1] / "examples" / "series-proposal.json"
MODEL = "factory-fixture"

# The production launcher ignores user configuration. The fixture shim forwards the
# original arguments to the real executable and supplies only the loopback provider.
WRAPPER = r'''
use std::{env, fs::{self, File}, process::{self, Command}};

fn main() {
    let executable = env::current_exe().unwrap();
    let root = executable.parent().unwrap();
    let native = fs::read_to_string(root.join("native.txt")).unwrap();
    let base = fs::read_to_string(root.join("base.txt")).unwrap();
    let status = Command::new(native)
        .args(["-c", "model_provider=\"factory_fixture\"", "-c", "model=\"unused-fixture-default\""])
        .args(["-c", "model_providers.factory_fixture.name=\"Factory fixture\""])
        .args(["-c", "model_providers.factory_fixture.wire_api=\"responses\""])
        .args(["-c", &format!("model_providers.factory_fixture.base_url=\"{base}\"")])
        .args(["-c", "model_providers.factory_fixture.request_max_retries=0"])
        .args(["-c", "model_providers.factory_fixture.stream_max_retries=0"])
        .args(env::args_os().skip(1))
        .stderr(File::create(root.join("native-stderr.txt")).unwrap())
        .status().unwrap();
    process::exit(status.code().unwrap_or(1));
}
'''


def item_stream(item, initial_item, response_id):
    usage = {"input_tokens": 100, "input_tokens_details": {"cached_tokens": 20}, "output_tokens": 50,
             "output_tokens_details": {"reasoning_tokens": 0}, "total_tokens": 150}
    events = [
        {"type": "response.output_item.added", "item": initial_item},
        {"type": "response.output_item.done", "item": item},
        {"type": "response.completed", "response": {"id": response_id, "usage": usage}},
    ]
    return "".join(f"event: {event['type']}\ndata: {json.dumps(event)}\n\n" for event in events).encode("utf-8")


def response_stream(suggestion, response_id="resp_fixture"):
    message = {"type": "message", "role": "assistant", "id": "msg_fixture",
               "content": [{"type": "output_text", "text": json.dumps(suggestion)}]}
    return item_stream(message, message | {"content": []}, response_id)


@contextmanager
def native_codex_fixture(codex, root, respond, *, hold_connection=None):
    requests = []

    class ResponsesFixture(BaseHTTPRequestHandler):
        def send_payload(self, payload, content_type):
            self.send_response(200)
            self.send_header("Content-Type", content_type)
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)

        def do_GET(self):
            if self.path.split("?", 1)[0] == "/v1/models":
                models = {"data": [{"id": MODEL, "object": "model", "created": 0, "owned_by": "fixture"}]}
                self.send_payload(json.dumps(models).encode("utf-8"), "application/json")
            else:
                self.send_error(404)

        def do_POST(self):
            if self.path != "/v1/responses" or self.headers.get("Authorization") is not None:
                self.send_error(403)
                return
            requests.append(json.loads(self.rfile.read(int(self.headers["Content-Length"]))))
            if hold_connection is not None:
                hold_connection(self.connection)
                return
            try:
                payload = respond(requests)
            except (KeyError, ValueError, RuntimeError) as error:
                print(f"Loopback model fixture rejected a request: {error}", file=sys.stderr)
                self.send_error(400, str(error))
                return
            self.send_payload(payload, "text/event-stream")

        def log_message(self, *_args):
            pass

    home = root / "isolated-home"
    home.mkdir()
    shim = root / ("codex-fixture.exe" if os.name == "nt" else "codex-fixture")
    source = root / "fixture.rs"
    source.write_text(WRAPPER, encoding="utf-8")
    (root / "native.txt").write_text(str(codex), encoding="utf-8")
    subprocess.run(["rustc", source, "-o", shim], check=True, timeout=30)

    with ThreadingHTTPServer(("127.0.0.1", 0), ResponsesFixture) as server:
        thread = Thread(target=server.serve_forever)
        thread.start()
        try:
            (root / "base.txt").write_text(f"http://127.0.0.1:{server.server_port}/v1", encoding="ascii")
            yield shim, home, requests
        finally:
            server.shutdown()
            thread.join()


def qualify(launcher, codex, root):
    seed = SEED.read_bytes()
    members = list(reversed(json.loads(seed)["members"]))
    work_ids = [member["work_order"]["id"] for member in members]
    suggestion = {
        "outcome": "proposed", "summary": "Stage the tolerant consumer before publishing the optional field.",
        "compatibility": "Check the consumer against both payload versions before publishing the field.",
        "rollback": "Disable the consumer flag before withdrawing the API observation.",
        "members": [{"work_id": work_id, "intent": "Verify this fixture member.",
                     "acceptance": ["The approved contract fixture passes."]} for work_id in work_ids],
    }
    profile = {"id": "fixture-planning", "model": MODEL, "reasoning_effort": "high", "timeout_seconds": 15,
               "max_attempts": 1, "approval_reference": "fixture:native-coordinator"}
    profile_bytes = json.dumps(profile).encode("utf-8")
    profile_digest = "sha256:" + sha256(profile_bytes).hexdigest()
    profile_path = root / "approved-profile.json"
    profile_path.write_bytes(profile_bytes)
    destination = root / "draft"
    with native_codex_fixture(codex, root, lambda _: response_stream(suggestion)) as (shim, home, requests):
        result = subprocess.run([
            launcher, "draft-series-proposal", SEED, "--seed-digest", "sha256:" + sha256(seed).hexdigest(),
            "--codex", shim, "--codex-home", home, "--destination", destination, "--timeout-seconds", "30",
            "--profile", profile_path, "--profile-digest", profile_digest,
        ], capture_output=True, text=True, timeout=40)
        if result.returncode:
            stderr = root / "native-stderr.txt"
            if stderr.is_file():
                print(stderr.read_text(encoding="utf-8", errors="replace")[-3000:], file=sys.stderr)
            raise RuntimeError(result.stderr.strip() or "Coordinator did not produce a draft")

    draft = json.loads(result.stdout)
    if (draft["state"] != "awaiting_approval"
            or draft["profile"] != profile or draft["profile_digest"] != profile_digest or draft["timeout_seconds"] != 15
            or json.loads((destination / "run.json").read_bytes()) != draft
            or draft["usage"] != {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 50}
            or (home / "auth.json").exists() or len(requests) != 1):
        raise RuntimeError("Unexpected state, usage, or authentication in the native coordinator fixture")
    request = requests[0]
    output_format = request["text"]["format"]
    if (request["model"] != MODEL or request.get("reasoning", {}).get("effort") != "high"
            or output_format["type"] != "json_schema"
            or "members" not in output_format["schema"]["properties"]
            or not all(work_id in json.dumps(request["input"]) for work_id in work_ids)):
        raise RuntimeError("Codex did not send the expected schema and planning seed")

    proposal = destination / "proposal.json"
    review = json.loads(subprocess.check_output([launcher, "review-series-proposal", proposal], timeout=15))
    if (review["proposal_digest"] != draft["proposal_digest"]
            or [row["member"]["work_order"]["id"] for row in review["members"]] != work_ids
            or review["compatibility"] != suggestion["compatibility"]):
        raise RuntimeError("The native result did not produce the expected reviewable proposal")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("launcher", type=Path)
    parser.add_argument("codex", type=Path)
    args = parser.parse_args()
    with TemporaryDirectory(prefix="craft native coordinator ") as scratch:
        qualify(args.launcher.resolve(strict=True), args.codex.resolve(strict=True), Path(scratch).resolve())
    print("Installed Codex passed loopback schema, draft, review, and usage checks.")


if __name__ == "__main__":
    main()
