"""Check the offline catalog-to-provider contract inventory; never execute cloud calls."""

import argparse
import copy
import hashlib
import json
from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parents[1]
CHANGE = ROOT / "openspec/changes/build-cloud-cli"
SNAPSHOT = CHANGE / "provider-operations.json"
METHODS = {"get", "post", "put", "patch", "delete", "head"}
READ_POSTS = {"searchAndReconsileIssuesUsingJqlPost", "getBulkPermissions"}
EFFECTS = {"read", "write", "trash", "permanent", "access_change", "unresolved"}
MOUNTS = {"jira-v3": "", "confluence-v1": "", "confluence-v2": "/wiki/api/v2"}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def closed(value, required, optional=()):
    require(isinstance(value, dict), "Expected a contract object")
    require(set(required) <= value.keys() <= set(required) | set(optional), "Unexpected or missing contract fields")


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, "Duplicate JSON object member")
        result[key] = value
    return result


def read_json(path, maximum=8_388_608):
    data = path.read_bytes()
    require(len(data) <= maximum, "Source exceeds the offline contract limit")
    return json.loads(data, object_pairs_hook=unique_object)


def digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()).hexdigest()


def pointer(root, path):
    require(isinstance(path, str) and path.startswith("#/"), "External native schema reference refused")
    current = root
    for part in path[2:].split("/"):
        part = part.replace("~1", "/").replace("~0", "~")
        current = current[int(part)] if isinstance(current, list) else current[part]
    return current


def escaped(value):
    return value.replace("~", "~0").replace("/", "~1")


def resolve(root, value, depth=0):
    require(depth < 64, "Native reference resolution exceeds its bound")
    return resolve(root, pointer(root, value["$ref"]), depth + 1) if "$ref" in value else value


def check_references(root, value):
    if isinstance(value, dict):
        for key, child in value.items():
            if key == "$ref":
                pointer(root, child)
            else:
                check_references(root, child)
    elif isinstance(value, list):
        for child in value:
            check_references(root, child)


def check_pin(data, source):
    require(len(data) == source["bytes"], "Pinned source byte length changed")
    require(hashlib.sha256(data).hexdigest() == source["sha256"], "Pinned source SHA-256 changed")


def load_sources():
    manifest = read_json(CHANGE / "provider-sources.json")
    closed(manifest, {"format_version", "sources"})
    require(manifest["format_version"] == 1, "Unsupported source manifest version")
    result = {}
    for source in manifest["sources"]:
        closed(source, {"id", "url", "path", "bytes", "sha256", "openapi", "version", "observed_on", "api_mount"})
        name = source["id"]
        require(name in MOUNTS and name not in result, "Unknown or duplicate provider specification")
        require(source["path"] == "sources/" + name + ".json", "Unexpected source path")
        require(source["api_mount"] == MOUNTS[name], "Untrusted API mount")
        require(source["url"].startswith("https://dac-static.atlassian.com/cloud/"), "Unexpected specification source origin")
        path = CHANGE / source["path"]
        check_pin(path.read_bytes(), source)
        spec = read_json(path)
        require(spec["openapi"] == source["openapi"] and spec["info"]["version"] == source["version"], "Native version pin mismatch")
        check_references(spec, spec)
        result[name] = {"pin": source, "spec": spec}
    require(set(result) == set(MOUNTS), "Incomplete provider source set")
    return result


def catalog_commands(text):
    commands = {}
    provider = None
    table = False
    for number, line in enumerate(text.splitlines(), 1):
        if line.startswith("## "):
            provider = {"## Jira Cloud": "jira", "## Confluence Cloud": "confluence"}.get(line)
            table = False
        elif line.startswith("### "):
            table = False
        if line.startswith("| Proposed command"):
            table = provider is not None
            continue
        if not table or not line.startswith("| ") or line.startswith("| ---"):
            continue
        cell = line.split("|")[1]
        literals = re.findall(r"`([^`]+)`", cell)
        require(literals, "Unrecognized command catalog row")
        for literal in literals:
            words = literal.split()
            require(len(words) == 2, "Catalog command requires an explicit resource and action")
            command = ".".join(words if cell.strip().startswith("Global ") else [provider, *words])
            require(command not in commands, "Duplicate catalog command")
            tiers = sorted({word.lower() for word in re.findall(r"\b(Core|Extended|Privileged|Conditional)\b", line)})
            require(tiers, "Catalog row has no release tier")
            commands[command] = {"line": number, "row_digest": hashlib.sha256(line.encode()).hexdigest(), "tiers": tiers}
    require(commands, "Empty command catalog")
    return commands


def operation_index(spec):
    result = {}
    for path, methods in spec["paths"].items():
        for method, operation in methods.items():
            if method not in METHODS:
                continue
            name = operation["operationId"]
            require(name not in result, "Duplicate native operation identity")
            result[name] = (path, method, operation, methods.get("parameters", []))
    return result


def value_matches(root, schema, value, depth=0):
    require(depth < 64, "Native constraint schema exceeds its bound")
    schema = resolve(root, schema)
    if "enum" in schema and value not in schema["enum"]:
        return False
    types = {"boolean": bool, "string": str, "array": list, "object": dict, "integer": int, "number": (int, float)}
    kind = schema.get("type")
    if kind in types and (not isinstance(value, types[kind]) or kind in {"integer", "number"} and isinstance(value, bool)):
        return False
    if kind == "array" and not all(value_matches(root, schema.get("items", {}), item, depth + 1) for item in value):
        return False
    for key in ["anyOf", "oneOf"]:
        if key in schema and not any(value_matches(root, branch, value, depth + 1) for branch in schema[key]):
            return False
    return all(value_matches(root, branch, value, depth + 1) for branch in schema.get("allOf", []))


def body_properties(root, schema, depth=0):
    require(depth < 64, "Native body schema exceeds its bound")
    schema = resolve(root, schema)
    result = {name: [value] for name, value in schema.get("properties", {}).items()}
    for union in ["allOf", "anyOf", "oneOf"]:
        for branch in schema.get(union, []):
            for name, values in body_properties(root, branch, depth + 1).items():
                result.setdefault(name, []).extend(values)
    return result


def check_constraints(spec, operation, path_parameters, variant):
    parameters = [resolve(spec, item) for item in [*path_parameters, *operation.get("parameters", [])]]
    query = {item["name"]: item for item in parameters if item["in"] == "query"}
    for name, value in variant["fixed_query"].items():
        require(name in query, "Fixed query parameter absent from native operation")
        require(value_matches(spec, query[name].get("schema", {}), value), "Fixed query parameter has an invalid native type/value")
    if variant["fixed_body"]:
        body = resolve(spec, operation.get("requestBody", {}))
        content = body.get("content", {}).get("application/json", {})
        properties = body_properties(spec, content.get("schema", {}))
        for name, value in variant["fixed_body"].items():
            require(name in properties, "Fixed body field absent from native schema")
            require(any(value_matches(spec, item, value) for item in properties[name]), "Fixed body field has an invalid native type/value")
    for name, value in variant["required_headers"].items():
        require(name == "X-Atlassian-Token" and value in {"nocheck", "no-check"}, "Unexpected fixed provider header")
        require(name in operation.get("description", "") and value in operation["description"], "Fixed header lacks native documentation")


def route_shape(spec, path, operation, path_parameters):
    require(path.startswith("/") and not path.startswith("//") and "?" not in path, "Native path must be product-relative")
    declared = [resolve(spec, item) for item in [*path_parameters, *operation.get("parameters", [])]]
    names = {item["name"] for item in declared if item["in"] == "path" and item.get("required") is True}
    segments = []
    found = set()
    for segment in path.strip("/").split("/"):
        match = re.fullmatch(r"\{([A-Za-z0-9_-]+)\}", segment)
        if match:
            name = match.group(1)
            require(name not in found, "Repeated native path parameter")
            found.add(name)
            segments.append({"kind": "parameter", "value": name})
        else:
            require(re.fullmatch(r"[A-Za-z0-9_.-]+", segment) and segment not in {".", ".."}, "Unsafe literal native path segment")
            segments.append({"kind": "literal", "value": segment})
    require(found == names, "Native path parameters do not match required declarations")
    query = [item["name"] for item in declared if item["in"] == "query"]
    require(len(query) == len(set(query)), "Duplicate native query name")
    return {"path": segments, "query_parameters": sorted(query)}


def lifecycle(command, variant):
    effect = variant["effect"]
    require(effect in EFFECTS, "Unknown lifecycle effect")
    query = variant["fixed_query"]
    if command in {"confluence.page.trash", "confluence.page.purge", "confluence.attachment.trash", "confluence.attachment.purge"}:
        purge = command.endswith(".purge")
        require(query.get("purge") is purge and effect == ("permanent" if purge else "trash"), "Trash/purge lifecycle mismatch")
        if ".page." in command:
            require(query.get("draft") is False, "Draft discard is not a trash/purge variant")
    if effect == "unresolved":
        require(variant.get("unavailable_reason"), "Unresolved lifecycle lacks an explicit unavailable reason")


def compile_matrix(config, sources, catalog_text, task_names):
    closed(config, {"format_version", "catalog", "commands", "deferred"})
    require(config["format_version"] == 1, "Unsupported mapping version")
    require(config["catalog"] == "../../../../docs/plans/cloud-cli-command-catalog.md", "Unexpected catalog path")
    catalog = catalog_commands(catalog_text)
    commands = [item["command"] for item in config["commands"]]
    require(len(commands) == len(set(commands)), "Duplicate mapped command")
    require(set(commands) == set(catalog), "Catalog coverage differs from mapped command set")
    indexes = {name: operation_index(source["spec"]) for name, source in sources.items()}
    output = []
    for item in sorted(config["commands"], key=lambda value: value["command"]):
        closed(item, {"command", "owner_task", "tier", "effect", "recovery", "retry", "conditions", "variants"})
        require(item["owner_task"] in task_names, "Mapped command has no owning task")
        require(item["tier"] in {"core", "extended", "privileged", "conditional"}, "Unknown command tier")
        require(item["tier"] in catalog[item["command"]]["tiers"], "Command tier does not match the catalog")
        require(item["effect"] in EFFECTS - {"unresolved"}, "Unknown command effect")
        require(bool(item["variants"]), "Mapped command has no native variant")
        if item["effect"] == "read":
            require(item["retry"] == "safe_read" and item["recovery"] == "not_applicable", "Read recovery metadata mismatch")
        else:
            require(item["retry"] == "reconcile_before_replay" and item["recovery"] in {"known_resource", "correlation", "async_job"}, "Mutation recovery metadata mismatch")
        compiled = copy.deepcopy(item)
        compiled["catalog_evidence"] = catalog[item["command"]]
        compiled["implemented"] = False
        compiled["runtime_available"] = False
        compiled["live_verified"] = False
        compiled["variants"] = []
        identities = set()
        for variant in item["variants"]:
            closed(variant, {"spec", "operation_id", "effect", "fixed_query", "fixed_body", "required_headers", "conditions"}, {"tier", "owner_task", "unavailable_reason"})
            name = variant["spec"]
            require(name in sources, "Unknown native specification")
            identity = (name, variant["operation_id"])
            require(identity not in identities, "Duplicate native command variant")
            identities.add(identity)
            require(variant.get("owner_task", item["owner_task"]) in task_names, "Native variant has no owning task")
            require(variant.get("tier", item["tier"]) in {"core", "extended", "privileged", "conditional"}, "Unknown native variant tier")
            require(variant.get("tier", item["tier"]) in catalog[item["command"]]["tiers"], "Native variant tier does not match the catalog")
            require(variant["operation_id"] in indexes[name], "Mapped native operation does not exist")
            path, method, operation, parameters = indexes[name][variant["operation_id"]]
            require(path.startswith("/rest/api/3/") if name == "jira-v3"
                    else path.startswith("/wiki/rest/api/") if name == "confluence-v1"
                    else path.startswith("/") and not path.startswith("/wiki/"), "Native path does not match its API family")
            require(variant["effect"] == item["effect"] or variant["effect"] == "unresolved", "Command/variant effect mismatch")
            native_read = method in {"get", "head"} or method == "post" and variant["operation_id"] in READ_POSTS
            require((variant["effect"] == "read") == native_read, "Effect does not match native read/mutation semantics")
            lifecycle(item["command"], variant)
            spec = sources[name]["spec"]
            check_constraints(spec, operation, parameters, variant)
            native_ref = "#/paths/" + escaped(path) + "/" + method
            scopes = []
            for alternative in operation.get("x-atlassian-oauth2-scopes", []):
                require(alternative.get("state") in {"Current", "Beta"}, "Unknown native scope lifecycle")
                require(isinstance(alternative.get("scopes"), list)
                        and all(isinstance(scope, str) and scope for scope in alternative["scopes"])
                        and len(set(alternative["scopes"])) == len(alternative["scopes"]), "Invalid native scope alternative")
                scheme = spec["components"]["securitySchemes"].get(alternative["scheme"], {})
                declared_scopes = scheme.get("flows", {}).get("authorizationCode", {}).get("scopes", {})
                require(scheme, "Native OAuth scheme is not defined")
                scopes.append({"state": alternative["state"], "scheme": alternative["scheme"],
                               "scopes": sorted(alternative["scopes"]),
                               "declared_in_security": any(
                                   sorted(requirement.get(alternative["scheme"], [])) == sorted(alternative["scopes"])
                                   and alternative["scheme"] in requirement
                                   for requirement in operation.get("security", [])),
                               "missing_scheme_scopes": sorted(set(alternative["scopes"]) - declared_scopes.keys())})
            description = operation.get("description", "")
            permission = re.search(r"\*\*\[Permissions\].*?required.*?:", description)
            permission_text = description[permission.end():].strip() if permission else None
            unavailable = ["not_implemented", "auth_unqualified", "tenant_unverified"]
            if operation.get("deprecated"):
                unavailable.append("deprecated_native_operation")
            if variant.get("unavailable_reason"):
                unavailable.append(variant["unavailable_reason"])
            if not any(alternative["state"] == "Current" for alternative in scopes):
                unavailable.append("current_oauth_scopes_unestablished")
            if any(alternative["state"] == "Current" and alternative["missing_scheme_scopes"] for alternative in scopes):
                unavailable.append("oauth_scope_definition_incomplete")
            if any(alternative["state"] == "Current" and not alternative["declared_in_security"] for alternative in scopes):
                unavailable.append("oauth_security_declaration_mismatch")
            if permission_text is None:
                unavailable.append("permission_evidence_unresolved")
            response = copy.deepcopy(variant)
            response.update({
                "method": method.upper(), "source_path": path, "route": MOUNTS[name] + path,
                "route_shape": route_shape(spec, MOUNTS[name] + path, operation, parameters),
                "source_ref": native_ref, "source_operation_sha256": digest(operation),
                "input": {"operation_parameters_ref": native_ref + "/parameters" if "parameters" in operation else None,
                          "path_parameters_ref": "#/paths/" + escaped(path) + "/parameters" if parameters else None,
                          "body_ref": native_ref + "/requestBody" if "requestBody" in operation else None},
                "output": {"responses_ref": native_ref + "/responses", "statuses": sorted(operation["responses"])},
                "oauth_scope_alternatives": scopes,
                "native_security_ref": native_ref + "/security" if "security" in operation else None,
                "permissions": {"description_ref": native_ref + "/description", "documented": permission_text is not None,
                                "text": permission_text[:2500] if permission_text else None,
                                "truncated": len(permission_text or "") > 2500},
                "app_access_rule_evidence": operation.get("x-atlassian-data-security-policy", []),
                "runtime_available": False, "unavailable_reasons": unavailable,
            })
            compiled["variants"].append(response)
        output.append(compiled)
    for deferred in config["deferred"]:
        closed(deferred, {"surface", "owners", "reason"})
        require(bool(deferred["reason"]) and set(deferred["owners"]) <= task_names, "Deferred surface lacks valid ownership/reason")
    return {"format_version": 1, "catalog_sha256": hashlib.sha256(catalog_text.encode()).hexdigest(),
            "sources": [source["pin"] for _, source in sorted(sources.items())], "commands": output,
            "deferred": copy.deepcopy(config["deferred"]), "runtime_available": False}


def inputs():
    sources = load_sources()
    config = read_json(CHANGE / "provider-catalog.json")
    catalog = (ROOT.parent / "docs/plans/cloud-cli-command-catalog.md").read_text(encoding="utf-8")
    tasks = set(re.findall(r"^### ([a-z0-9-]+)$", (CHANGE / "tasks.md").read_text(encoding="utf-8"), re.M))
    return config, sources, catalog, tasks


def selftest(config, sources, catalog, tasks):
    baseline = compile_matrix(config, sources, catalog, tasks)
    controls = 0
    def rejects(mutator):
        nonlocal controls
        candidate = copy.deepcopy(config)
        mutator(candidate)
        try:
            compile_matrix(candidate, sources, catalog, tasks)
        except (ValueError, KeyError, TypeError):
            controls += 1
            return
        raise ValueError("Negative provider control was accepted")
    rejects(lambda value: value["commands"].pop())
    rejects(lambda value: value["commands"].append(copy.deepcopy(value["commands"][0])))
    rejects(lambda value: value["commands"][0].update(owner_task="missing-owner"))
    rejects(lambda value: value["commands"][0]["variants"][0].update(operation_id="missing-operation"))
    rejects(lambda value: value["commands"][0].update(runtime_available=True))
    rejects(lambda value: value["commands"][0]["variants"][0].update(effect="write"))
    read = next(index for index, value in enumerate(config["commands"]) if value["command"] == "jira.issue.get")
    rejects(lambda value: value["commands"][read]["variants"][0].update(operation_id="createIssue"))
    rejects(lambda value: value["commands"][read].update(tier="privileged"))
    mutation = next(index for index, value in enumerate(config["commands"]) if value["command"] == "jira.issue.create")
    rejects(lambda value: value["commands"][mutation].update(recovery="not_applicable"))
    rejects(lambda value: value["commands"][mutation]["variants"][0].update(operation_id="getIssue"))
    trash = next(index for index, value in enumerate(config["commands"]) if value["command"] == "confluence.page.trash")
    rejects(lambda value: value["commands"][trash]["variants"][0]["fixed_query"].update(purge=True))
    rejects(lambda value: value["commands"][trash]["variants"][0]["fixed_query"].update(draft=True))
    rejects(lambda value: value["commands"][read]["variants"][0]["fixed_query"].update(unknown=True))
    rejects(lambda value: value["commands"][read]["variants"][0]["fixed_query"].update(fields=True))
    try:
        check_pin(b"changed source", sources["jira-v3"]["pin"])
    except ValueError:
        controls += 1
    else:
        raise ValueError("Source pin negative control was accepted")
    try:
        pointer({}, "https://untrusted.invalid/schema")
    except ValueError:
        controls += 1
    else:
        raise ValueError("External reference negative control was accepted")
    altered = copy.deepcopy(sources)
    native = altered["jira-v3"]["spec"]["paths"]["/rest/api/3/issue/{issueIdOrKey}"]["get"]
    native["x-atlassian-oauth2-scopes"][0]["scopes"].append("read:project:jira")
    require(compile_matrix(config, altered, catalog, tasks) != baseline, "Scope drift did not change the matrix")
    controls += 1
    altered = copy.deepcopy(sources)
    native = altered["jira-v3"]["spec"]["paths"]["/rest/api/3/issue/{issueIdOrKey}"]["get"]
    native.pop("x-atlassian-oauth2-scopes")
    unresolved = compile_matrix(config, altered, catalog, tasks)
    selected = next(row for row in unresolved["commands"] if row["command"] == "jira.issue.get")
    require("current_oauth_scopes_unestablished" in selected["variants"][0]["unavailable_reasons"], "Missing scopes were inferred")
    controls += 1
    altered_config = copy.deepcopy(config)
    selected = next(row for row in altered_config["commands"] if row["command"] == "jira.version.delete")
    selected["variants"][0]["operation_id"] = "deleteVersion"
    deprecated = compile_matrix(altered_config, sources, catalog, tasks)
    selected = next(row for row in deprecated["commands"] if row["command"] == "jira.version.delete")
    require("deprecated_native_operation" in selected["variants"][0]["unavailable_reasons"], "Deprecated route was not identified")
    controls += 1
    selected = next(row for row in baseline["commands"] if row["command"] == "jira.issue.create")
    alternatives = selected["variants"][0]["oauth_scope_alternatives"]
    require(any(item["state"] == "Current" and item["scopes"] == ["write:jira-work"] for item in alternatives)
            and any(item["state"] == "Beta" and "write:issue:jira" in item["scopes"] for item in alternatives), "Scope lifecycles were merged")
    controls += 1
    selected = next(row for row in baseline["commands"] if row["command"] == "jira.permission.check")
    require(any(alternative["scopes"] == [] for variant in selected["variants"]
                for alternative in variant["oauth_scope_alternatives"]), "Explicit empty OAuth scope alternative was lost")
    controls += 1
    selected = next(row for row in baseline["commands"] if row["command"] == "confluence.attachment.purge")
    require("oauth_scope_definition_incomplete" in selected["variants"][0]["unavailable_reasons"], "Native scope-definition discrepancy was hidden")
    controls += 1
    selected = next(row for row in baseline["commands"] if row["command"] == "confluence.page.move")
    require("permission_evidence_unresolved" in selected["variants"][0]["unavailable_reasons"], "Absent permission evidence was inferred")
    controls += 1
    source = sources["jira-v3"]["pin"]
    data = (CHANGE / source["path"]).read_bytes()
    try:
        check_pin(bytes([data[0] ^ 1]) + data[1:], source)
    except ValueError:
        controls += 1
    else:
        raise ValueError("Same-length source corruption was accepted")
    print(json.dumps({"ok": True, "controls": controls}))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["check", "write", "selftest", "show"])
    parser.add_argument("--command")
    arguments = parser.parse_args()
    try:
        config, sources, catalog, tasks = inputs()
        if arguments.action == "selftest":
            selftest(config, sources, catalog, tasks)
            return 0
        result = compile_matrix(config, sources, catalog, tasks)
        if arguments.action == "write":
            pending = SNAPSHOT.with_suffix(".pending")
            pending.write_text(json.dumps(result, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
            pending.replace(SNAPSHOT)
        elif arguments.action == "check":
            require(SNAPSHOT.exists() and read_json(SNAPSHOT) == result, "Provider matrix changed; inspect and regenerate intentionally")
        else:
            require(arguments.command is not None, "Show requires --command")
            selected = next((row for row in result["commands"] if row["command"] == arguments.command), None)
            require(selected is not None, "Unknown mapped command")
            encoded = json.dumps(selected, indent=2, ensure_ascii=False)
            require(len(encoded.encode()) <= 32_768, "Selected provider contract exceeds the display bound; inspect its source references")
            print(encoded)
            return 0
        print(json.dumps({"ok": True, "commands": len(result["commands"]),
                          "variants": sum(len(row["variants"]) for row in result["commands"]),
                          "sources": len(result["sources"]), "runtime_available": False}))
        return 0
    except (OSError, ValueError, KeyError, TypeError, RecursionError) as error:
        print(json.dumps({"ok": False, "error": str(error)[:500]}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
