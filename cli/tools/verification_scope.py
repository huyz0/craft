"""Select checks from changed paths; a focused pass is not full qualification."""

HOSTS = [
    "confluence_context_host", "jira_create_host", "jira_project_list_host",
    "jira_issue_reads_host", "jira_project_context_host", "jira_field_reads_host",
    "jira_issue_context_host", "jira_permissions_host", "jira_issue_mutation_host",
]
FLOWS = {
    "confluence": {"confluence_context_host"},
    "issue_create": {"jira_create_host"},
    "planning_review": {"confluence_context_host", "jira_create_host"},
    "issue_recovery": {"jira_create_host", "jira_issue_mutation_host"},
    "saved_requests": {"jira_create_host", "jira_issue_mutation_host"},
    "issue_mutation": {"jira_issue_mutation_host"},
    "issue_reads": {"jira_issue_reads_host"},
    "issue_context": {"jira_issue_context_host"},
    "issue_comments": {"jira_issue_reads_host"},
    "attachment_reads": {"jira_issue_reads_host"},
    "project_context": {"jira_project_context_host"},
    "project_list": {"jira_project_list_host"},
    "field_reads": {"jira_field_reads_host"},
    "permission_reads": {"jira_permissions_host"},
    "policies": set(), "provision": set(), "auth": set(), "planning": set(),
}
SHARED_CLI = {"args.rs", "dispatch.rs", "lib.rs", "catalog.rs", "contracts.rs"}


def select(paths, force_full=False):
    paths = sorted(set(path.replace("\\", "/") for path in paths))
    result = {"scope": "unchanged", "packages": [], "hosts": [], "checks": []}
    if not paths and not force_full:
        return result
    packages, hosts, checks = set(), set(), {"planning"}
    full = force_full
    shared_cli = False
    for path in paths:
        parts = path.split("/")
        if path.endswith(".md") and not path.startswith("crates/"):
            if path.startswith("skills/"):
                checks.add("runtime-skill")
            continue
        if path in {"Cargo.toml", "Cargo.lock", "rust-toolchain.toml"} or path.startswith(".cargo/") or path.endswith("/Cargo.toml"):
            full = True
        elif path.startswith("crates/"):
            package = parts[1]
            if package in {"craft-core", "craft-runtime", "craft-platform", "craft-test-support"}:
                full = True
            elif package == "craft-provider-confluence":
                checks.update({"providers", "contracts"})
                packages.update({package, "craft-cli"})
                hosts.add("confluence_context_host")
            elif package == "craft-provider-jira":
                checks.update({"providers", "contracts"})
                packages.update({package, "craft-cli"})
                hosts.update(name for name in HOSTS if name.startswith("jira_"))
            elif package == "craft-workflows":
                packages.update({package, "craft-cli"})
                checks.add("contracts")
            elif package == "craft-cli":
                packages.add(package)
                relative = "/".join(parts[2:])
                if relative.startswith("tests/"):
                    if parts[-1].removesuffix(".rs") in HOSTS:
                        hosts.add(parts[-1].removesuffix(".rs"))
                    elif relative.startswith("tests/confluence/"):
                        hosts.add("confluence_context_host")
                    elif relative.startswith("tests/jira_"):
                        matches = {name for name in HOSTS if any(
                            relative.startswith("tests/" + directory + "/")
                            for directory in {name, name.removesuffix("_host")})}
                        hosts.update(matches or {name for name in HOSTS if name.startswith("jira_")})
                    elif len(parts) > 4:
                        hosts.update(HOSTS)
                elif relative.startswith("src/"):
                    if parts[-1] == "tests.rs" or "/tests/" in relative:
                        continue
                    checks.add("contracts")
                    module = parts[3].removesuffix(".rs")
                    flow = "confluence" if module.startswith("confluence") else module
                    if flow in FLOWS:
                        hosts.update(FLOWS[flow])
                    elif parts[3] in SHARED_CLI:
                        shared_cli = True
                        checks.add("contracts")
                    elif module in {"main", "output"}:
                        hosts.update(HOSTS)
                        checks.add("contracts")
                    else:
                        hosts.update(HOSTS)
                else:
                    checks.add("contracts")
            else:
                full = True
        elif path.startswith("tools/"):
            name = parts[-1]
            if name in {"verify.py", "verification_scope.py"}:
                checks.add("verifier")
            elif name == "plan.py":
                checks.add("planning-selftest")
            elif name == "contracts.py":
                checks.add("contracts")
            elif name in {"runtime_skill.py", "skills.py", "skills_test.py", "skills_qualify.py",
                         "skills_qualify_test.py", "release.py", "release_test.py"}:
                checks.add("runtime-skill")
            elif name in {"bundle.py", "bundle_test.py"}:
                checks.add("bundle")
            elif name == "dependencies.py":
                checks.add("dependencies")
            elif name == "providers.py":
                checks.add("providers")
            elif name == "jira_conformance.py":
                checks.add("jira-conformance")
            elif name == "synthetic-acceptance.py":
                checks.add("isolation")
            else:
                full = True
        elif path.startswith("schemas/"):
            checks.add("contracts")
            packages.add("craft-cli")
        elif path.startswith("skills/"):
            checks.add("runtime-skill")
        elif path == "docs/evidence/dependency-inventory.json":
            checks.add("dependencies")
        elif path.endswith(("provider-operations.json", "provider-sources.json")) or "/sources/" in path:
            checks.add("providers")
            packages.update({"craft-provider-jira", "craft-provider-confluence", "craft-cli"})
            hosts.update(HOSTS)
        elif path.endswith((".md", ".json")) and (path.startswith(("docs/", "openspec/", ".agents/")) or "/" not in path):
            pass
        else:
            full = True
    if shared_cli:
        hosts.update(HOSTS)
    if "contracts" in checks or full:
        checks.add("runtime-skill")
    result.update(scope="full" if full else "focused" if packages or checks != {"planning"} else "planning",
                  packages=sorted(packages), hosts=sorted(hosts), checks=sorted(checks))
    return result


def selftest():
    assert select([])["scope"] == "unchanged"
    docs = select(["docs/evidence/result.md", "STATE.md"])
    assert docs["scope"] == "planning" and not docs["packages"] and not docs["hosts"]
    assert select(["Cargo.lock"])["scope"] == "full"
    assert select(["crates/craft-runtime/src/auth/service.rs"])["scope"] == "full"
    assert select(["crates/new-provider/src/lib.rs"])["scope"] == "full"
    assert select([], force_full=True)["scope"] == "full"
    confluence = select(["crates/craft-provider-confluence/src/pages/mod.rs"])
    assert confluence["packages"] == ["craft-cli", "craft-provider-confluence"]
    assert confluence["hosts"] == ["confluence_context_host"]
    assert {"providers", "contracts"}.issubset(confluence["checks"])
    cli = select(["crates/craft-cli/src/args.rs", "crates/craft-cli/src/confluence_pages/command.rs"])
    assert cli["hosts"] == sorted(HOSTS)
    assert select(["crates/craft-cli/src/dispatch.rs"])["hosts"] == sorted(HOSTS)
    policy = select(["crates/craft-cli/src/policies.rs", "crates/craft-cli/src/args.rs"])
    assert policy["packages"] == ["craft-cli"] and policy["hosts"] == sorted(HOSTS)
    assert "contracts" in select(["crates/craft-cli/src/profiles.rs"])["checks"]
    workflow = select(["crates/craft-workflows/src/plans/mod.rs"])
    assert workflow["packages"] == ["craft-cli", "craft-workflows"]
    assert "contracts" in workflow["checks"] and workflow["hosts"] == []
    assert "dependencies" in select(["docs/evidence/dependency-inventory.json"])["checks"]
    assert select(["crates/craft-cli/tests/confluence/publication.rs"])["hosts"] == ["confluence_context_host"]
    assert select(["crates/craft-cli/tests/jira_issue_context/some_case.rs"])["hosts"] == ["jira_issue_context_host"]
    assert select(["crates/craft-cli/tests/jira_create_host/reviewed_create.rs"])["hosts"] == ["jira_create_host"]
    reviewed = select(["crates/craft-cli/src/planning_review/verification.rs", "crates/craft-cli/src/issue_create/material.rs"])
    assert reviewed["hosts"] == ["confluence_context_host", "jira_create_host"]
    assert select(["crates/craft-cli/src/planning_review/verification.rs", "crates/craft-cli/src/dispatch.rs"])["hosts"] == sorted(HOSTS)
    assert select(["crates/craft-cli/tests/jira_mutation/actions.rs"])["hosts"]
    assert not select(["crates/craft-cli/src/dispatch/tests.rs"])["hosts"]
    assert select(["crates/craft-provider-confluence/tests/fixtures/page.md"])["packages"]
    assert select(["tools/verification_scope.py"])["checks"] == ["planning", "verifier"]
    assert select(["tools/README.md"])["scope"] == "planning"
    skill = select(["skills/craft-cloud/SKILL.md", "skills/craft-cloud/compatibility.json"])
    assert skill["checks"] == ["planning", "runtime-skill"] and not skill["hosts"] and not skill["packages"]
    assert "runtime-skill" in select(["schemas/local-contracts.json"])["checks"]
    assert select(["crates/craft-cli/src/saved_requests.rs"])["hosts"] == ["jira_create_host", "jira_issue_mutation_host"]
    assert select(["crates/craft-cli/src/issue_comments.rs"])["hosts"] == ["jira_issue_reads_host"]
    assert select(["crates/craft-cli/src/attachment_reads.rs"])["hosts"] == ["jira_issue_reads_host"]
    package = select(["tools/bundle.py", "tools/bundle_test.py"])
    assert package["checks"] == ["bundle", "planning"] and not package["hosts"] and not package["packages"]
