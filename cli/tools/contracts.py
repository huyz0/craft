"""Generate and check reviewed contracts from Craft's actual parser and registry."""

import argparse
from copy import deepcopy
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile


ROOT = Path(__file__).resolve().parents[1]
SNAPSHOT = ROOT / "schemas/local-contracts.json"
DESCRIPTOR_FIELDS = {
    "id", "provider", "contract_version", "api_revision", "description", "effect",
    "input_schema", "response_schema", "example", "response_example", "projection",
    "scope_alternatives", "permissions", "tenant_conditions", "routes", "retry_class",
    "reconciliation",
}
OPERATION_FIELDS = {
    "path", "descriptor", "flags", "controls", "selectable_fields", "default_projection_digest",
}


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("Duplicate contract property")
        result[key] = value
    return result


def read_json(text):
    return json.loads(text, object_pairs_hook=unique_object)


def generate():
    result = subprocess.run(
        ["cargo", "run", "--quiet", "--locked", "--offline", "-p", "craft-cli", "--example", "contracts"],
        cwd=ROOT, capture_output=True, text=True, encoding="utf-8", check=False,
    )
    if result.returncode:
        raise ValueError("Contract exporter failed: " + result.stderr[-4000:])
    if len(result.stdout.encode("utf-8")) > 8 * 1024 * 1024:
        raise ValueError("Contract export exceeds the development snapshot bound")
    document = read_json(result.stdout)
    validate(document)
    return document


def require(condition, path):
    if not condition:
        raise ValueError("Incomplete or inconsistent contract at " + path)


def validate(document):
    require(isinstance(document, dict), "/")
    require(set(document) == {"format_version", "contract_version", "controls", "providers", "operations"}, "/")
    require(type(document["format_version"]) is int and document["format_version"] == 1, "/format_version")
    require(isinstance(document["contract_version"], str) and document["contract_version"], "/contract_version")
    require(isinstance(document["controls"], dict) and document["controls"], "/controls")
    require(isinstance(document["providers"], list), "/providers")
    require(isinstance(document["operations"], dict) and document["operations"], "/operations")
    provider_ids = {"craft"}
    for provider in document["providers"]:
        require(isinstance(provider, dict) and set(provider) == {"id", "version", "deployments", "contract_version"}, "/providers")
        require(provider["id"] not in provider_ids, "/providers/id")
        provider_ids.add(provider["id"])
    for command, operation in document["operations"].items():
        path = "/operations/" + command
        require(re.fullmatch(r"[a-z][a-z0-9-]*(?:\.[a-z][a-z0-9-]*)*", command), path)
        require(isinstance(operation, dict) and set(operation) == OPERATION_FIELDS, path)
        require(operation["path"] == command.split("."), path + "/path")
        descriptor = operation["descriptor"]
        require(isinstance(descriptor, dict) and set(descriptor) == DESCRIPTOR_FIELDS, path + "/descriptor")
        require(descriptor["id"] == command, path + "/descriptor/id")
        require(descriptor["provider"] in provider_ids, path + "/descriptor/provider")
        require(descriptor["contract_version"] == document["contract_version"], path + "/descriptor/contract_version")
        for field in ["description", "api_revision"]:
            require(isinstance(descriptor[field], str) and descriptor[field].strip(), path + "/descriptor/" + field)
        require(descriptor["effect"] in {"read", "write", "trash", "permanent", "access_change"}, path + "/descriptor/effect")
        require(descriptor["retry_class"] in {"local_read", "safe_read", "never", "reconcile_before_replay"}, path + "/descriptor/retry_class")
        require(descriptor["reconciliation"] in {"not_applicable", "known_resource", "correlation"}, path + "/descriptor/reconciliation")
        if descriptor["effect"] != "read":
            require(descriptor["retry_class"] in {"never", "reconcile_before_replay"}, path + "/descriptor/retry_class")
            require(descriptor["reconciliation"] != "not_applicable", path + "/descriptor/reconciliation")
        if descriptor["retry_class"] == "local_read":
            require(descriptor["provider"] == "craft" and not descriptor["routes"], path + "/descriptor/retry_class")
        scopes = descriptor["scope_alternatives"]
        require(isinstance(scopes, list), path + "/descriptor/scope_alternatives")
        combinations = set()
        for alternative in scopes:
            require(isinstance(alternative, list) and alternative, path + "/descriptor/scope_alternatives")
            require(all(isinstance(scope, str) and scope and not any(char.isspace() for char in scope) for scope in alternative), path + "/descriptor/scope_alternatives")
            require(len(set(alternative)) == len(alternative), path + "/descriptor/scope_alternatives")
            normalized = tuple(sorted(alternative))
            require(normalized not in combinations, path + "/descriptor/scope_alternatives")
            combinations.add(normalized)
        for field in ["permissions", "tenant_conditions"]:
            require(isinstance(descriptor[field], list) and all(isinstance(value, str) and value.strip() for value in descriptor[field]), path + "/descriptor/" + field)
        require(isinstance(descriptor["routes"], list), path + "/descriptor/routes")
        for field in ["input_schema", "response_schema", "example", "response_example"]:
            require(isinstance(descriptor[field], dict), path + "/descriptor/" + field)
        require(isinstance(operation["flags"], dict), path + "/flags")
        require(isinstance(operation["controls"], dict) and set(operation["controls"]) == set(document["controls"]), path + "/controls")
        require(set(operation["controls"].values()) <= {"supported", "rejected"}, path + "/controls")
        names = set()
        for field, flag in {**document["controls"], **operation["flags"]}.items():
            require(isinstance(flag, dict) and isinstance(flag.get("long"), str) and flag["long"], path + "/flags/" + field)
            require(flag["long"] not in names, path + "/flags/" + field)
            names.add(flag["long"])
        require(isinstance(operation["selectable_fields"], dict), path + "/selectable_fields")
        if descriptor["projection"] is not None:
            require(bool(operation["selectable_fields"]), path + "/selectable_fields")
            require(isinstance(operation["default_projection_digest"], str), path + "/default_projection_digest")
        else:
            require(not operation["selectable_fields"] and operation["default_projection_digest"] is None, path + "/selectable_fields")


def differences(previous, current, path=""):
    if type(previous) is not type(current):
        yield path or "/"
    elif isinstance(previous, dict):
        for key in sorted(set(previous) | set(current)):
            child = path + "/" + key.replace("~", "~0").replace("/", "~1")
            if key not in previous or key not in current:
                yield child
            else:
                yield from differences(previous[key], current[key], child)
    elif previous != current:
        yield path or "/"


def write_snapshot(document, target):
    validate(document)
    target.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(mode="w", encoding="utf-8", dir=target.parent,
                                     prefix="contract-write-", suffix=".tmp", delete=False) as stream:
        pending = Path(stream.name)
        stream.write(json.dumps(document, ensure_ascii=False, indent=2) + "\n")
    try:
        pending.replace(target)
    finally:
        pending.unlink(missing_ok=True)


def check(document, target):
    previous = read_json(target.read_text(encoding="utf-8"))
    validate(previous)
    changes = list(differences(previous, document))
    if changes:
        return {"ok": False, "changed_paths": changes[:12], "more": len(changes) > 12,
                "next_action": "Review compatibility at these paths; run python tools/contracts.py write only for an intentional contract change."}
    return {"ok": True, "operations": len(document["operations"]), "snapshot": str(target.relative_to(ROOT))}


def selftest(document):
    controls = 0
    mutations = [
        ("flag", lambda value: value["operations"]["command.find"]["flags"]["query"].update(long="different-query")),
        ("response", lambda value: value["operations"]["version"]["descriptor"]["response_schema"]["properties"]["data"]["properties"]["version"].update(type="integer")),
        ("projection", lambda value: value["operations"]["version"]["selectable_fields"].pop("version")),
    ]
    temporary_root = ROOT / "target/tmp"
    temporary_root.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="contract-controls-", dir=temporary_root) as directory:
        target = Path(directory) / "contracts.json"
        write_snapshot(document, target)
        require(check(document, target)["ok"], "valid_snapshot")
        controls += 1
        for name, mutate in mutations:
            changed = deepcopy(document)
            mutate(changed)
            write_snapshot(changed, target)
            require(not check(document, target)["ok"], name + "_drift_control")
            write_snapshot(document, target)
            require(check(document, target)["ok"], name + "_regeneration")
            controls += 1
        for field in ["scope_alternatives", "retry_class", "reconciliation"]:
            changed = deepcopy(document)
            changed["operations"]["version"]["descriptor"].pop(field)
            try:
                write_snapshot(changed, target)
            except ValueError:
                controls += 1
            else:
                raise ValueError("Incomplete regenerated descriptor was accepted")
        for field, value in [("retry_class", "safe_read"), ("reconciliation", "not_applicable")]:
            changed = deepcopy(document)
            changed["operations"]["profile.create"]["descriptor"][field] = value
            try:
                validate(changed)
            except ValueError:
                controls += 1
            else:
                raise ValueError("Unsafe mutation metadata was accepted")
    try:
        read_json('{"property":true,"property":false}')
    except ValueError:
        controls += 1
    else:
        raise ValueError("Duplicate snapshot property was accepted")
    return {"ok": True, "checks": controls}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["check", "write", "selftest"], default="check", nargs="?")
    arguments = parser.parse_args()
    try:
        document = generate()
        if arguments.action == "selftest":
            result = selftest(document)
        elif arguments.action == "write":
            write_snapshot(document, SNAPSHOT)
            result = {"ok": True, "written": str(SNAPSHOT.relative_to(ROOT)), "operations": len(document["operations"])}
        else:
            result = check(document, SNAPSHOT)
        print(json.dumps(result, ensure_ascii=False))
        return 0 if result["ok"] else 1
    except (OSError, ValueError, KeyError, TypeError) as error:
        print(json.dumps({"ok": False, "error": str(error)}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
