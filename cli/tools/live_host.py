"""Drive the real CLI host against the live test site with the provisioned service grant.

Opt-in companion to the live conformance crate. It never handles secrets: the
service credential was stored by `craft auth provision`, and this script only
invokes the built executable with an explicit state root and profiles.

Local, untracked inputs:
  cli/.craft-live.json   {"site": "https://<name>.atlassian.net", "service_actor": "<account id>"}
  state root             cli/target/live-state (override with --state-root)

Commands:
  policies   create or update the local allowances the live runs need
  reads      run the public read commands through the full host and report
"""

import argparse
import json
import subprocess
import sys
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CRAFT = ROOT / "target" / "debug" / ("craft.exe" if sys.platform == "win32" else "craft")
CAPTURES = ROOT / "target" / "live-captures"


def config():
    data = json.loads((ROOT / ".craft-live.json").read_text(encoding="utf-8"))
    site = data["site"].rstrip("/")
    with urllib.request.urlopen(f"{site}/_edge/tenant_info", timeout=30) as response:
        cloud = json.load(response)["cloudId"]
    return site, cloud, data.get("service_actor")


def craft(state_root, args, input_value=None):
    command = [str(CRAFT), "--state-root", str(state_root), *args]
    path = None
    if input_value is not None:
        CAPTURES.mkdir(parents=True, exist_ok=True)
        path = CAPTURES / "host-input.json"
        path.write_text(json.dumps(input_value), encoding="utf-8")
        command += ["--input", str(path)]
    done = subprocess.run(command, capture_output=True, text=True, encoding="utf-8", cwd=ROOT)
    try:
        return json.loads(done.stdout)
    except json.JSONDecodeError:
        return {"ok": False, "error": {"code": "NOT_JSON", "message": (done.stdout + done.stderr)[:400]}}


def ref(provider, cloud, kind, ident):
    return {"provider": provider, "deployment": "cloud", "tenant": cloud, "resource_type": kind, "id": ident}


def policies(state_root, site, cloud, actor, project, space):
    plans = [
        ("jira-test", "jira-read", ref("jira", cloud, "site", cloud), [
            "jira.issue.get", "jira.issue.search", "jira.metadata.edit", "jira.transition.list",
            "jira.permission.check", "jira.link.get", "jira.link.list", "jira.link.types",
            "jira.comment.get", "jira.comment.list", "jira.attachment.get", "jira.attachment.list",
            "jira.attachment.settings"], None),
        ("jira-test", "jira-project-read", ref("jira", cloud, "site", cloud), ["jira.project.list"], site),
        ("jira-test", "jira-project-context", ref("jira", cloud, "project", project),
         ["jira.project.get", "jira.project.statuses"], None),
        ("jira-test", "jira-field-read", ref("jira", cloud, "site", cloud),
         ["jira.field.list", "jira.field.search"], site),
        ("jira-test", "jira-create", ref("jira", cloud, "project", project), ["jira.issue.create"], None),
        ("jira-test", "jira-mutation", ref("jira", cloud, "project", project), [
            "jira.issue.update", "jira.transition.apply", "jira.issue.assign", "jira.issue.unassign",
            "jira.issue.delete"], None),
        ("cf-test", "confluence-write", ref("confluence", cloud, "space", space),
         ["confluence.page.create", "confluence.page.update", "confluence.page.trash",
          "confluence.comment.create", "confluence.comment.update", "confluence.comment.delete"], None),
        ("cf-test", "confluence-read", ref("confluence", cloud, "site", cloud), [
            "confluence.space.get", "confluence.space.list", "confluence.page.get", "confluence.page.list",
            "confluence.page.search", "confluence.comment.get", "confluence.comment.list",
            "confluence.comment.replies"], None),
    ]
    results = []
    for profile, kind, target, operations, continuation in plans:
        current = craft(state_root, ["--profile", profile, "policy", "get"], {"kind": kind, "target": target})
        revision = ((current.get("data") or {}).get("policy") or {}).get("revision")
        request = {"kind": kind, "target": target, "expected_actor": actor, "enabled": True,
                   "operations": operations, "expected_revision": revision, "apply": True}
        if continuation:
            request["continuation_site"] = continuation
        out = craft(state_root, ["--profile", profile, "policy", "set"], request)
        results.append((kind, out.get("ok"), out.get("error")))
    return results


def run(state_root, profile, command, value):
    out = craft(state_root, ["--profile", profile, *command.split(".")], value)
    return out


def first(out, *keys):
    items = (out.get("data") or {}).get("items") or []
    if not items:
        return None
    value = items[0]
    for key in keys:
        value = value.get(key) if isinstance(value, dict) else None
    return value


def reads(state_root, cloud):
    """Runs read commands through the full host. Identifiers come from earlier reads."""
    report = []

    def step(profile, command, value):
        out = run(state_root, profile, command, value)
        error = out.get("error") or {}
        detail = "" if out.get("ok") else f" {error.get('code')}: {error.get('message')} {json.dumps(error.get('details'))[:300]}"
        report.append(f"{command} => {'PASS' if out.get('ok') else 'FAIL'}{detail}")
        return out

    projects = step("jira-test", "jira.project.list", {"page_size": 50})
    project_id, project_key = first(projects, "id"), first(projects, "key")
    step("jira-test", "jira.field.list", {})
    step("jira-test", "jira.field.search", {"page_size": 50})
    step("jira-test", "jira.attachment.settings", {})
    step("jira-test", "jira.link.types", {})
    if project_key:
        # Project-context policies bind the exact numeric project target.
        step("jira-test", "jira.project.get", {"project": project_id})
        step("jira-test", "jira.project.statuses", {"project": project_id})
        search = step("jira-test", "jira.issue.search", {"jql": f"project = {project_key} ORDER BY key", "page_size": 50})
        issue = first(search, "id")
        if issue:
            step("jira-test", "jira.issue.get", {"issue": issue})
            step("jira-test", "jira.metadata.edit", {"issue": issue})
            step("jira-test", "jira.transition.list", {"issue": issue})
            step("jira-test", "jira.permission.check", {"issue": issue, "permissions": ["BROWSE_PROJECTS", "EDIT_ISSUES"]})
            step("jira-test", "jira.comment.list", {"issue_id": issue})
            step("jira-test", "jira.attachment.list", {"issue_id": issue})
            step("jira-test", "jira.link.list", {"issue_id": issue})
    spaces = step("cf-test", "confluence.space.list", {"page_size": 25})
    space = first(spaces, "id")
    if space:
        step("cf-test", "confluence.space.get", {"id": space})
    pages = step("cf-test", "confluence.page.list", {"page_size": 25})
    page = first(pages, "id")
    step("cf-test", "confluence.page.search", {"cql": "type = page", "page_size": 25})
    if page:
        step("cf-test", "confluence.page.get", {"id": page})
        step("cf-test", "confluence.comment.list", {"page_id": page})
    return report


def expected_state(state_root, issue):
    out = craft(state_root, ["--profile", "jira-test", "jira", "issue", "get",
                             "--fields", "id,key,project.id,issue_type.id,status.id,updated"], {"issue": issue})
    i = (out.get("data") or {}).get("issue") or {}
    return {"id": i.get("id"), "key": i.get("key"), "project_id": (i.get("project") or {}).get("id"),
            "issue_type_id": (i.get("issue_type") or {}).get("id"), "status_id": (i.get("status") or {}).get("id"),
            "updated": i.get("updated")}


def delete_issue(state_root, issue, operation_id):
    return run(state_root, "jira-test", "jira.issue.delete", {
        "operation_id": operation_id, "issue": issue, "expected": expected_state(state_root, issue),
        "subtasks": {"mode": "refuse"}, "apply": True})


def writes(state_root, project, issue_type):
    """Creates one craft-test issue through the full host, transitions and edits it, then deletes it."""
    import time
    stamp = int(time.time() * 1000)
    report = []

    def step(name, out):
        error = out.get("error") or {}
        detail = "" if out.get("ok") else f" {error.get('code')}: {error.get('message')} {json.dumps(error.get('details'))[:400]}"
        report.append(f"{name} => {'PASS' if out.get('ok') else 'FAIL'}{detail}")
        return out

    created = step("jira.issue.create", run(state_root, "jira-test", "jira.issue.create", {
        "operation_id": f"craft-test-{stamp}-create", "project_id": project, "issue_type_id": issue_type,
        "summary": f"craft-test host {stamp}", "apply": True}))
    data = created.get("data") or {}
    issue = data.get("issue_id") or (data.get("issue") or {}).get("id") or next(
        (r.get("id") for r in (data.get("receipt") or {}).get("confirmed_resources", [])
         if r.get("resource_type") == "issue"), None)
    if not issue:
        report.append(f"create returned no issue id: {json.dumps(data)[:400]}")
        return report
    try:
        transitions = step("jira.transition.list", run(state_root, "jira-test", "jira.transition.list", {"issue": issue}))
        items = (transitions.get("data") or {}).get("items") or []
        target = next((t for t in items if t.get("to", {}).get("id") != expected_state(state_root, issue)["status_id"]), None)
        if target:
            step("jira.transition.apply", run(state_root, "jira-test", "jira.transition.apply", {
                "operation_id": f"craft-test-{stamp}-transition", "issue": issue,
                "expected": expected_state(state_root, issue), "transition_id": target["id"],
                "expected_status_id": target.get("to", {}).get("id"), "changes": {}, "apply": True}))
        else:
            report.append(f"jira.transition.apply => SKIP no transition found {json.dumps(items)[:300]}")
        step("jira.issue.update", run(state_root, "jira-test", "jira.issue.update", {
            "operation_id": f"craft-test-{stamp}-update", "issue": issue, "expected": expected_state(state_root, issue),
            "changes": {"summary": f"craft-test host {stamp} edited"}, "apply": True}))
    finally:
        step("jira.issue.delete", delete_issue(state_root, issue, f"craft-test-{stamp}-delete"))
    return report


def page_lifecycle(state_root, space):
    """Creates one craft-test page through the full host, updates it with the explicit editor-draft
    policy, and checks a stale revision is refused. tests/live_cleanup.rs removes the page."""
    import time
    stamp = int(time.time() * 1000)
    report = []

    def step(name, out, expect_ok=True, expect_code=None):
        error = out.get("error") or {}
        passed = out.get("ok") if expect_ok else (not out.get("ok") and error.get("code") == expect_code)
        detail = "" if passed else f" {error.get('code')}: {error.get('message')} {json.dumps(error.get('details'))[:400]}"
        report.append(f"{name} => {'PASS' if passed else 'FAIL'}{detail}")
        return out

    created = step("confluence.page.create", run(state_root, "cf-test", "confluence.page.create", {
        "operation_id": f"craft-test-{stamp}-page", "apply": True, "space_id": space,
        "title": f"craft-test host page {stamp}", "placement": {"kind": "root"},
        "body": {"representation": "storage", "value": "<p>craft-test page created through the CLI host.</p>"}}))
    resources = ((created.get("data") or {}).get("receipt") or {}).get("confirmed_resources") or []
    page = next((r.get("id") for r in resources if r.get("resource_type") == "page"), None)
    if not page:
        return report

    def revision():
        out = step("confluence.page.get", craft(state_root, [
            "--profile", "cf-test", "confluence", "page", "get", "--fields", "id,version,revision"], {"id": page}))
        return ((out.get("data") or {}).get("item") or {}).get("revision")

    first = revision()
    policy = "native_reconciliation_may_overwrite"
    updated = step("confluence.page.update", run(state_root, "cf-test", "confluence.page.update", {
        "operation_id": f"craft-test-{stamp}-update", "apply": True, "space_id": space, "id": page,
        "expected": first, "editor_draft_policy": policy,
        "changes": {"title": f"craft-test host page {stamp} edited",
                    "body": {"representation": "storage", "value": "<p>craft-test page updated.</p>"}}}))
    data = updated.get("data") or {}
    report.append(f"update receipt version={data.get('version')} policy={data.get('editor_draft_policy')} "
                  f"preservation_verified={data.get('editor_draft_preservation_verified')}")
    second = revision()
    report.append(f"revision {first and first.get('version')} -> {second and second.get('version')}")
    step("confluence.page.update stale revision", run(state_root, "cf-test", "confluence.page.update", {
        "operation_id": f"craft-test-{stamp}-stale", "apply": True, "space_id": space, "id": page,
        "expected": first, "editor_draft_policy": policy,
        "changes": {"title": f"craft-test host page {stamp} stale"}}), expect_ok=False, expect_code="VERSION_CONFLICT")
    if "--comments" in sys.argv:
        body = {"representation": "storage", "value": "<p>craft-test comment</p>"}
        made = step("confluence.comment.create", run(state_root, "cf-test", "confluence.comment.create", {
            "operation_id": f"craft-test-{stamp}-comment", "apply": True, "space_id": space, "page_id": page,
            "body": body}))
        found = ((made.get("data") or {}).get("receipt") or {}).get("confirmed_resources") or []
        comment = next((r.get("id") for r in found if r.get("resource_type") == "comment"), None)
        if comment:
            step("confluence.comment.create reply", run(state_root, "cf-test", "confluence.comment.create", {
                "operation_id": f"craft-test-{stamp}-reply", "apply": True, "space_id": space, "page_id": page,
                "parent_comment_id": comment, "body": body}))

            def comment_revision():
                out = step("confluence.comment.get", craft(state_root, [
                    "--profile", "cf-test", "confluence", "comment", "get", "--fields", "id,version,revision"],
                    {"id": comment, "page_id": page}))
                return ((out.get("data") or {}).get("item") or {}).get("revision")

            original = comment_revision()
            step("confluence.comment.update", run(state_root, "cf-test", "confluence.comment.update", {
                "operation_id": f"craft-test-{stamp}-comment-edit", "apply": True, "space_id": space,
                "page_id": page, "id": comment, "expected": original,
                "body": {"representation": "storage", "value": "<p>craft-test comment edited</p>"}}))
            step("confluence.comment.delete stale revision", run(state_root, "cf-test", "confluence.comment.delete", {
                "operation_id": f"craft-test-{stamp}-comment-stale", "apply": True, "space_id": space,
                "page_id": page, "id": comment, "expected": original}), expect_ok=False, expect_code="VERSION_CONFLICT")
            step("confluence.comment.delete", run(state_root, "cf-test", "confluence.comment.delete", {
                "operation_id": f"craft-test-{stamp}-comment-delete", "apply": True, "space_id": space,
                "page_id": page, "id": comment, "expected": comment_revision()}))
    if "--trash" in sys.argv:
        step("confluence.page.trash", run(state_root, "cf-test", "confluence.page.trash", {
            "operation_id": f"craft-test-{stamp}-trash", "apply": True, "space_id": space, "id": page,
            "expected": second}))
    return report


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("command", choices=["policies", "reads", "writes", "page"])
    parser.add_argument("--issue-type", help="numeric non-subtask issue type id for writes")
    parser.add_argument("--state-root", default=str(ROOT / "target" / "live-state"))
    parser.add_argument("--project", help="numeric Jira project id")
    parser.add_argument("--space", help="numeric Confluence space id")
    parser.add_argument("--trash", action="store_true", help="also trash the page (needs delete:page:confluence)")
    parser.add_argument("--comments", action="store_true", help="also create, reply, edit and delete footer comments")
    args = parser.parse_args()
    site, cloud, actor = config()
    if not actor:
        sys.exit("add service_actor to .craft-live.json")
    if args.command == "policies":
        for kind, ok, error in policies(args.state_root, site, cloud, actor, args.project, args.space):
            print(kind, "ok" if ok else f"FAIL {json.dumps(error)[:600]}")
    else:
        if args.command == "reads":
            report = reads(args.state_root, cloud)
        elif args.command == "writes":
            report = writes(args.state_root, args.project, args.issue_type)
        else:
            report = page_lifecycle(args.state_root, args.space)
        text = "\n".join(report)
        print(text)
        CAPTURES.mkdir(parents=True, exist_ok=True)
        (CAPTURES / "host-reads-summary.txt").write_text(text, encoding="utf-8")


if __name__ == "__main__":
    main()
