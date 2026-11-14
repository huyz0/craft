"""Check the repository runtime skill offline; optionally measure actual tokenizer lengths."""

import argparse
import hashlib
import json
from pathlib import Path
import re
import shlex
import sys
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parents[1]
SKILL = ROOT / "skills/craft-cloud"
CONTRACT = ROOT / "schemas/local-contracts.json"


def require(condition, message):
    if not condition:
        raise ValueError(message)


def scalar_argument(raw, schema, root):
    """Check documented scalar flags, not arbitrary JSON documents or file contents."""
    if "$ref" in schema:
        reference = schema["$ref"]
        require(reference.startswith("#/$defs/"), "Unsupported flag schema reference")
        return scalar_argument(raw, root["$defs"][reference.removeprefix("#/$defs/")], root)
    if "anyOf" in schema:
        for variant in schema["anyOf"]:
            try:
                return scalar_argument(raw, variant, root)
            except ValueError:
                pass
        raise ValueError("Flag does not match any input variant")
    kinds = schema.get("type")
    kinds = kinds if isinstance(kinds, list) else [kinds]
    value = raw
    if "integer" in kinds:
        require(isinstance(raw, str) and re.fullmatch(r"-?[0-9]+", raw), "Expected integer flag")
        value = int(raw)
        require(schema.get("minimum", value) <= value <= schema.get("maximum", value),
                "Integer flag outside schema range")
    elif "string" in kinds:
        require(isinstance(raw, str), "Expected string flag")
        require(schema.get("minLength", 0) <= len(raw) <= schema.get("maxLength", len(raw)),
                "String flag outside schema length")
        require("pattern" not in schema or re.search(schema["pattern"], raw), "Flag pattern mismatch")
    elif "boolean" in kinds:
        require(raw is None, "Expected boolean switch")
        value = True
    else:
        raise ValueError("Unsupported scalar flag schema; use a validated input-file fixture")
    require("enum" not in schema or value in schema["enum"], "Flag enum mismatch")
    return value


def command_example(line, contract):
    words = shlex.split(line)
    require(words and words.pop(0) == "craft", "Expected a Craft example")
    path = []
    while words and not words[0].startswith("-"):
        path.append(words.pop(0))
    command = ".".join(path)
    require(command in contract["operations"], "Unknown example command: " + command)
    operation = contract["operations"][command]
    flags = {value["long"]: value for value in operation["flags"].values()}
    flags.update({value["long"]: value for key, value in contract["controls"].items()
                  if operation["controls"].get(key) == "supported"})
    selected = {}
    while words:
        flag = words.pop(0)
        require(flag.startswith("--") and flag[2:] in flags, "Unknown example flag: " + flag)
        spec = flags[flag[2:]]
        value = None
        if spec["arity"]["max"]:
            require(words and not words[0].startswith("--"), "Missing value: " + flag)
            value = words.pop(0)
            require(not spec["choices"] or value in spec["choices"], "Invalid choice: " + flag)
        selected[flag[2:]] = value
    if "fields" in selected:
        require(set(selected["fields"].split(",")) <= set(operation["selectable_fields"]),
                "Unknown selected field in " + command)
    if command in {"schema", "input.validate"}:
        require(selected.get("command") in contract["operations"], "Unknown schema target")
    action_flags = {value["long"]: key for key, value in operation["flags"].items()}
    schema = operation["descriptor"]["input_schema"]
    action_keys = {action_flags[flag] for flag in selected if flag in action_flags}
    if "input" in selected:
        require(not action_keys, "Input files cannot be mixed with action flags")
        # File names in instructions are operator-provided inputs, not bundled fixtures.
    else:
        require(set(schema.get("required", [])) <= action_keys, "Missing required example action input")
        for flag, key in action_flags.items():
            if flag in selected:
                scalar_argument(selected[flag], schema["properties"][key], schema)
    for flag in {"max-bytes", "timeout-ms"} & selected.keys():
        require(re.fullmatch(r"[0-9]+", selected[flag]) and int(selected[flag]) > 0,
                "Expected positive numeric control: " + flag)
    return command


def check(directory=SKILL, snapshot=CONTRACT):
    contract_bytes = snapshot.read_bytes()
    contract = json.loads(contract_bytes)
    compatibility = json.loads((directory / "compatibility.json").read_text(encoding="utf-8"))
    # Hash checked-in text, not checkout line endings, so CRLF and LF worktrees agree.
    normalized = contract_bytes.replace(b"\r\n", b"\n")
    require(compatibility["contract_snapshot_sha256"] == hashlib.sha256(normalized).hexdigest(),
            "Skill compatibility snapshot changed; review instructions before rebinding")
    require(compatibility["contract_version"] == contract["contract_version"], "Contract version mismatch")
    cargo = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
    require(compatibility["binary_version"] == cargo["workspace"]["package"]["version"],
            "Binary version mismatch")
    require(set(compatibility["providers"]) <= {p["id"] for p in contract["providers"]}, "Unknown provider")
    require(compatibility["status"] == "development"
            and set(compatibility["hosts"].values()) == {"unqualified"},
            "Host qualification requires separate evidence and checker update")
    entry = (directory / "SKILL.md").read_text(encoding="utf-8")
    frontmatter = re.match(r"\A---\nname: ([a-z0-9-]+)\ndescription: ([^\n]+)\n---\n", entry)
    require(frontmatter and frontmatter[1] == directory.name, "Invalid two-field skill frontmatter")
    commands = set()
    files = sorted(directory.rglob("*.md"))
    for path in files:
        text = path.read_text(encoding="utf-8")
        for target in re.findall(r"\[[^\]]+\]\(([^)]+)\)", text):
            resolved = (path.parent / target).resolve()
            require(resolved.is_relative_to(directory.resolve()) and resolved.is_file(),
                    "Broken or escaping skill link: " + target)
        for cited in re.findall(r"`([a-z][a-z0-9-]*(?:\.[a-z][a-z0-9-]*)+)`", text):
            require(cited in contract["operations"] or cited.endswith(".json"), "Unknown cited command: " + cited)
            if cited in contract["operations"]:
                commands.add(cited)
        for line in text.splitlines():
            if line.startswith("craft "):
                commands.add(command_example(line, contract))
    return {"ok": True, "markdown_files": len(files), "commands_checked": sorted(commands),
            "host_qualification": "not_run", "validation_network_requests": 0}


def measure():
    import tiktoken

    results = {}
    for name in ["cl100k_base", "o200k_base"]:
        encoding = tiktoken.get_encoding(name)
        lengths = {}
        for path in sorted(SKILL.rglob("*.md")):
            text = path.read_text(encoding="utf-8")
            tokens = encoding.encode(text, disallowed_special=())
            require(encoding.decode(tokens) == text, "Tokenizer roundtrip failed")
            lengths[path.relative_to(SKILL).as_posix()] = len(tokens)
        results[name] = lengths
    return {"tokenizer": "tiktoken", "version": tiktoken.__version__, "files": results,
            "interpretation": "Text lengths only; no Claude tokenizer or task-efficiency claim"}


def selftest():
    contract = json.loads(CONTRACT.read_bytes())
    command_example('craft schema --command confluence.page.get --view full', contract)
    invalid = [
        'craft command search --query page',
        'craft schema --command confluence.page.get --view minimal',
        'craft confluence page get --id 123 --fields absent',
        'craft schema --command missing.command',
        'craft schema --command',
        'craft confluence page get --fields id,title',
        'craft confluence page get --id 0 --fields id,title',
        'craft command find --query page --limit bananas',
        'craft command find --query page --limit 0',
        'craft confluence page get --id 123 --input page.json',
        'craft confluence page get --id 123 --max-bytes bananas',
    ]
    for line in invalid:
        try:
            command_example(line, contract)
        except ValueError:
            continue
        raise ValueError("Invalid example accepted: " + line)
    temporary_root = ROOT / "target/tmp"
    temporary_root.mkdir(parents=True, exist_ok=True)
    import shutil

    with tempfile.TemporaryDirectory(dir=temporary_root) as temporary:
        copy = Path(temporary) / SKILL.name
        shutil.copytree(SKILL, copy)
        check(copy)
        entry = copy / "SKILL.md"
        original = entry.read_text(encoding="utf-8")
        for target in ["missing.md", "../../outside.md"]:
            entry.write_text(original + f"\n[reference]({target})\n", encoding="utf-8")
            try:
                check(copy)
            except ValueError:
                continue
            raise ValueError("Invalid reference accepted")
        entry.write_text(original, encoding="utf-8")
        manifest = copy / "compatibility.json"
        value = json.loads(manifest.read_bytes())
        value["contract_snapshot_sha256"] = "0" * 64
        manifest.write_text(json.dumps(value), encoding="utf-8")
        try:
            check(copy)
        except ValueError:
            pass
        else:
            raise ValueError("Stale compatibility accepted")
    return {"ok": True, "negative_cases": len(invalid) + 3}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=["check", "selftest", "tokens"], default="check", nargs="?")
    arguments = parser.parse_args()
    try:
        result = selftest() if arguments.mode == "selftest" else check()
        if arguments.mode == "tokens":
            result["measurements"] = measure()
        print(json.dumps(result, indent=2))
        return 0
    except (OSError, ValueError, KeyError, ImportError) as error:
        print(json.dumps({"ok": False, "error": str(error)}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
