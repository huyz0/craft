"""Record and verify the qualification receipt for a Factory release kit."""

import argparse
from hashlib import file_digest, sha256
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tarfile

from release_kit_extract import PLATFORM_ARCHIVES, archive_record, install_archive


RECEIPT = "qualification.json"
RELEASE_MANIFEST = "release.json"
SCHEMA_VERSION = 7
NATIVE_COMPARISON = "native-worker-comparison.json"
WINDOWS_SMOKE_RESULT = {
    "schema_version": 1,
    "passed": True,
    "migrations": "installed",
    "preflight": "passed",
    "queue": "paused",
    "operator_tls": "verified",
    "runner_operator_access": "denied",
    "candidate_handoff": "independently verified",
    "confluence_drift_report": "verified",
}
COMMON = ("launcher_tests", "extracted_kit", "ci_repair")
KITS = {
    PLATFORM_ARCHIVES["windows"]: ("Windows", "offline", COMMON + ("queue_smoke", "queue_https")),
    PLATFORM_ARCHIVES["linux"]: (
        "Linux", "shared-queue", COMMON + ("queue_sql", "queue_https", "queue_smoke", "native_workers"),
    ),
}


def kit(archive):
    if archive.name not in KITS:
        raise ValueError("Unrecognized Factory release archive name")
    return KITS[archive.name]


def commit(value):
    return isinstance(value, str) and re.fullmatch(r"[0-9a-f]{40}", value) is not None


def run_number(value):
    if not re.fullmatch(r"[1-9][0-9]*", value):
        raise ValueError("Workflow run ID and attempt must be positive integers")
    return int(value)


def native_comparison(archive):
    report = archive.with_name(NATIVE_COMPARISON)
    if report.is_symlink() or not report.is_file():
        raise ValueError("The Linux native worker comparison is missing")
    with report.open("rb") as stream:
        contents = stream.read(256 * 1024 + 1)
    if len(contents) > 256 * 1024:
        raise ValueError("The Linux native worker comparison exceeds its size limit")
    data = json.loads(contents)
    if (not isinstance(data, dict) or type(data.get("schema_version")) is not int
            or data["schema_version"] != 1 or data.get("model_responses") != "scripted_loopback"
            or not commit(data.get("base_commit")) or not commit(data.get("verified_tree"))):
        raise ValueError("Unsupported Linux native worker comparison format")
    comparison = data.get("comparison")
    results = comparison.get("results") if isinstance(comparison, dict) else None
    if (not isinstance(results, list) or len(results) != 2
            or any(not isinstance(row, dict) or row.get("independent_result") != "passed"
                   or not commit(row.get("head")) or type(row.get("verification_sequence")) is not int
                   or row["verification_sequence"] <= 0 for row in results)
            or sorted(str(row.get("adapter")) for row in results) != ["claude", "codex"]):
        raise ValueError("The Linux native worker comparison requires a passing verdict for both adapters")

    fields = {"craft-factory": "launcher_sha256", "shared/craft-factory-queue": "queue_sha256"}
    found = set()
    try:
        with tarfile.open(archive, "r|gz") as kit:
            for member in kit:
                if member.name not in fields:
                    continue
                if member.name in found or not member.isfile():
                    raise ValueError("The Linux release archive has ambiguous executables")
                with kit.extractfile(member) as binary:
                    if file_digest(binary, "sha256").hexdigest() != data.get(fields[member.name]):
                        raise ValueError("The native worker comparison identifies different Linux executables")
                found.add(member.name)
    except tarfile.TarError as error:
        raise ValueError("Could not read the Linux archive for native worker comparison") from error
    if found != fields.keys():
        raise ValueError("The Linux release archive is missing an executable checked by the native workers")
    return {"name": report.name, "sha256": sha256(contents).hexdigest(), "bytes": len(contents)}


def validate_windows_smoke(evidence, archive_info):
    if not isinstance(evidence, dict) or evidence.keys() != {"archive_sha256", "result"}:
        raise ValueError("The Windows packaged smoke report is missing or malformed")
    if evidence["archive_sha256"] != archive_info["sha256"]:
        raise ValueError("The Windows packaged smoke report identifies a different archive")
    result = evidence["result"]
    if (not isinstance(result, dict) or type(result.get("schema_version")) is not int
            or result.get("passed") is not True or result != WINDOWS_SMOKE_RESULT):
        raise ValueError("The Windows packaged smoke report must include the successful Confluence scenario")


def write(archive, *, windows_smoke_report=None):
    platform, job, required = kit(archive)
    if (os.environ["GITHUB_EVENT_NAME"], os.environ["KIT_PROFILE"], os.environ["GITHUB_JOB"],
            os.environ["RUNNER_OS"], os.environ["RUNNER_ARCH"]) != ("workflow_dispatch", "release", job, platform, "X64"):
        raise ValueError("The release kit receipt requires the matching manual release job")
    source = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=os.environ["GITHUB_WORKSPACE"],
                                     text=True, timeout=10).strip()
    workflow = os.environ["GITHUB_WORKFLOW_SHA"]
    if not commit(source) or source != os.environ["GITHUB_SHA"] or not commit(workflow):
        raise ValueError("The checked-out source and workflow must identify the release revision")
    checks = {name: os.environ.get("KIT_CHECK_" + name.upper()) for name in required}
    for name, outcome in checks.items():
        if outcome != "success":
            raise ValueError(f"The {name} qualification did not succeed")
    archive_info = archive_record(archive)
    receipt = {"schema_version": SCHEMA_VERSION, "archive": archive_info, "source_commit": source, "profile": "release",
               "platform": platform, "architecture": "X64", "checks": checks,
               "github": {"repository": os.environ["GITHUB_REPOSITORY"], "job": job, "workflow_commit": workflow,
                          "run_id": run_number(os.environ["GITHUB_RUN_ID"]),
                          "run_attempt": run_number(os.environ["GITHUB_RUN_ATTEMPT"])}}
    if platform == "Linux":
        if windows_smoke_report is not None:
            raise ValueError("A Linux receipt cannot include a Windows packaged smoke report")
        receipt["native_comparison"] = native_comparison(archive)
    else:
        if windows_smoke_report is None or windows_smoke_report.is_symlink() or not windows_smoke_report.is_file():
            raise ValueError("The Windows packaged smoke report is missing or malformed")
        with windows_smoke_report.open("rb") as stream:
            smoke = stream.read(4097)
        if len(smoke) > 4096:
            raise ValueError("The Windows packaged smoke report exceeds its size limit")
        evidence = json.loads(smoke)
        validate_windows_smoke(evidence, archive_info)
        receipt["windows_smoke"] = evidence
    archive.with_name(RECEIPT).write_text(json.dumps(receipt, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    archive.with_name("SHA256SUMS").write_bytes(f"{archive_info['sha256']}  {archive.name}\n".encode("ascii"))


def verify(archive, source, repository, run_id, run_attempt):
    platform, job, required = kit(archive)
    receipt = json.loads(archive.with_name(RECEIPT).read_text(encoding="utf-8"))
    if not isinstance(receipt, dict):
        raise ValueError("Unsupported qualification receipt format")
    version = receipt.get("schema_version")
    if type(version) is not int or not 1 <= version <= SCHEMA_VERSION:
        raise ValueError("Unsupported qualification receipt format")
    if version < 4:
        required = tuple(name for name in required if name != "native_workers")
    if version < 7 and platform == "Windows":
        required = tuple(name for name in required if name != "queue_https")
    if version == 1 or (version == 2 and platform == "Windows"):
        required = tuple(name for name in required if name != "queue_smoke")
    if (receipt.get("profile"), receipt.get("platform"), receipt.get("architecture")) != ("release", platform, "X64"):
        raise ValueError("The qualification receipt is for a different platform or build profile")
    if not commit(source) or receipt.get("source_commit") != source:
        raise ValueError("The qualification receipt is for a different source revision")
    github = receipt.get("github")
    if (not isinstance(github, dict) or not commit(github.get("workflow_commit"))
            or type(github.get("run_id")) is not int or type(github.get("run_attempt")) is not int
            or (github.get("repository"), github.get("job"), github.get("run_id"), github.get("run_attempt"))
            != (repository, job, run_id, run_attempt)):
        raise ValueError("The qualification receipt is for a different workflow run or attempt")
    if receipt.get("checks") != dict.fromkeys(required, "success"):
        raise ValueError("The qualification receipt is missing required successful checks")
    archive_info = archive_record(archive)
    if receipt.get("archive") != archive_info:
        raise ValueError("The archive does not match the qualification receipt")
    if archive.with_name("SHA256SUMS").read_text(encoding="ascii") != f"{archive_info['sha256']}  {archive.name}\n":
        raise ValueError("SHA256SUMS does not match the qualified archive")
    if version >= 5 and platform == "Linux":
        if receipt.get("native_comparison") != native_comparison(archive):
            raise ValueError("The native worker comparison does not match the qualification receipt")
    elif "native_comparison" in receipt:
        raise ValueError("Unexpected native worker comparison in the qualification receipt")
    if version >= 6 and platform == "Windows":
        validate_windows_smoke(receipt.get("windows_smoke"), archive_info)
    elif "windows_smoke" in receipt:
        raise ValueError("Unexpected Windows packaged smoke evidence in the qualification receipt")
    return receipt


def bundle_manifest(directory, source, repository, run_id, run_attempt, workflow, *, minimum_receipt=3):
    if not commit(workflow):
        raise ValueError("A bundle requires the expected full workflow commit")
    platforms = {platform.lower() for platform, _, _ in KITS.values()}
    if {entry.name for entry in directory.iterdir()} - {RELEASE_MANIFEST} != platforms:
        raise ValueError("A Factory release contains exactly the Windows and Linux kit directories")
    packages = {}
    for filename, (platform, _, _) in KITS.items():
        relative = Path(platform.lower()) / filename
        archive = directory / relative
        if archive.parent.is_symlink() or not archive.parent.is_dir():
            raise ValueError(f"{platform} release kit requires a platform directory")
        files = list(archive.parent.iterdir())
        names = {entry.name for entry in files}
        required = {filename, RECEIPT, "SHA256SUMS"}
        allowed = required | ({NATIVE_COMPARISON} if platform == "Linux" else set())
        if (not required <= names <= allowed
                or any(entry.is_symlink() or not entry.is_file() for entry in files)):
            raise ValueError(f"{platform} release kit requires exactly its archive, receipt, and checksum")
        receipt = verify(archive, source, repository, run_id, run_attempt)
        if receipt["schema_version"] < minimum_receipt or receipt["github"]["workflow_commit"] != workflow:
            raise ValueError(f"{platform} qualification requires the bundle workflow and receipt version {minimum_receipt} or later")
        packages[platform.lower()] = {
            "path": relative.as_posix(),
            "sha256": receipt["archive"]["sha256"],
            "bytes": receipt["archive"]["bytes"],
            "qualification_sha256": archive_record(archive.with_name(RECEIPT))["sha256"],
        }
        if "native_comparison" in receipt:
            packages["linux"]["native_comparison"] = receipt["native_comparison"]
        elif NATIVE_COMPARISON in names:
            raise ValueError("Unexpected native worker comparison beside a historical receipt")
    return {"schema_version": 1, "source_commit": source, "workflow_commit": workflow, "repository": repository,
            "run_id": run_id, "run_attempt": run_attempt, "packages": packages}


def bundle(directory, source, repository, run_id, run_attempt, workflow):
    manifest = bundle_manifest(directory, source, repository, run_id, run_attempt, workflow, minimum_receipt=SCHEMA_VERSION)
    with directory.joinpath(RELEASE_MANIFEST).open("x", encoding="utf-8") as output:
        json.dump(manifest, output, indent=2, sort_keys=True)
        output.write("\n")


def verify_bundle(directory, source, repository, run_id, run_attempt, workflow):
    path = directory / RELEASE_MANIFEST
    if path.is_symlink() or not path.is_file():
        raise ValueError("The Factory release manifest is missing")
    declared = json.loads(path.read_text(encoding="utf-8"))
    expected = bundle_manifest(directory, source, repository, run_id, run_attempt, workflow)
    if not isinstance(declared, dict) or type(declared.get("schema_version")) is not int or declared != expected:
        raise ValueError("Release manifest does not match both qualified archives and receipts")
    return expected


def install_bundle(directory, source, repository, run_id, run_attempt, workflow, *, platform, destination):
    manifest = verify_bundle(directory, source, repository, run_id, run_attempt, workflow)
    kit = manifest["packages"][platform]
    archive = directory / kit["path"]
    expected = {"name": archive.name, "sha256": kit["sha256"], "bytes": kit["bytes"]}
    install_archive(archive, platform, destination, expected)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="action", required=True)
    writer = commands.add_parser("write")
    writer.add_argument("archive", type=Path)
    writer.add_argument("--windows-smoke-report", type=Path)
    verifier = commands.add_parser("verify")
    verifier.add_argument("archive", type=Path)
    bundle_writer = commands.add_parser("bundle")
    bundle_reader = commands.add_parser("verify-bundle")
    installer = commands.add_parser("install-bundle", help="verify a release and extract one kit to a new directory")
    for reader in (bundle_writer, bundle_reader, installer):
        reader.add_argument("directory", type=Path)
        reader.add_argument("--workflow-commit", required=True)
    installer.add_argument("destination", type=Path)
    installer.add_argument("--platform", choices=("windows", "linux"),
                           default={"win32": "windows", "linux": "linux"}.get(sys.platform))
    for reader in (verifier, bundle_writer, bundle_reader, installer):
        reader.add_argument("--source-commit", required=True)
        reader.add_argument("--repository", required=True)
        reader.add_argument("--run-id", required=True, type=run_number)
        reader.add_argument("--run-attempt", required=True, type=run_number)
    args = parser.parse_args()
    if args.action == "install-bundle" and args.platform is None:
        parser.error("Specify --platform when preparing a kit on a different operating system")
    try:
        if args.action == "write":
            write(args.archive, windows_smoke_report=args.windows_smoke_report)
        elif args.action == "verify":
            result = verify(args.archive, args.source_commit, args.repository, args.run_id, args.run_attempt)
            print(f"Verified {args.archive.name} sha256:{result['archive']['sha256']} "
                  f"(qualification receipt v{result['schema_version']})")
        else:
            binding = (args.directory, args.source_commit, args.repository, args.run_id, args.run_attempt, args.workflow_commit)
            if args.action == "bundle":
                bundle(*binding)
            elif args.action == "install-bundle":
                install_bundle(*binding, platform=args.platform, destination=args.destination)
                print(f"Installed verified {args.platform} kit in {args.destination}")
                return
            else:
                verify_bundle(*binding)
            print(f"Verified both Factory release kits for workflow run {args.run_id}, attempt {args.run_attempt}")
    except (EOFError, KeyError, OSError, ValueError, subprocess.SubprocessError) as error:
        parser.exit(1, f"Release kit receipt: {error}\n")


if __name__ == "__main__":
    main()
