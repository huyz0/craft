"""Check the document converter against a pinned, explicitly fetched ADF schema."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import urllib.request


ROOT = Path(__file__).resolve().parents[1]
SOURCE = "https://unpkg.com/@atlaskit/adf-schema@57.7.39/dist/json-schema/v1/full.json"
DIGEST = "5128562b75278c8a83e7e3619a570205bc80d59696985ec31a7a7883cff66fbe"
MAXIMUM = 1_048_576


def checked(data):
    if len(data) > MAXIMUM or hashlib.sha256(data).hexdigest() != DIGEST:
        raise ValueError("ADF schema fixture differs from the qualified vendor version")
    json.loads(data)
    return data


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fetch", action="store_true", help="Fetch the pinned public schema before checking")
    arguments = parser.parse_args()
    destination = ROOT / "target" / "vendor-adf-schema.json"
    try:
        if arguments.fetch:
            with urllib.request.urlopen(SOURCE, timeout=20) as response:
                data = checked(response.read(MAXIMUM + 1))
            destination.parent.mkdir(parents=True, exist_ok=True)
            temporary = destination.with_suffix(".pending")
            temporary.write_bytes(data)
            temporary.replace(destination)
        with destination.open("rb") as source:
            data = checked(source.read(MAXIMUM + 1))
        environment = dict(os.environ, CRAFT_ADF_SCHEMA=str(destination))
        subprocess.run([
            "cargo", "test", "-p", "craft-workflows", "--locked", "--offline", "--",
            "--ignored", "--exact",
            "documents::tests::rendered_documents_validate_against_the_fetched_atlassian_schema",
            "--nocapture",
        ], cwd=ROOT, env=environment, check=True, timeout=120)
        print(json.dumps({"ok": True, "source": SOURCE, "bytes": len(data), "sha256": DIGEST}))
        return 0
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        print(json.dumps({"ok": False, "error": str(error)}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
