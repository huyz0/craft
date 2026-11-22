"""Run SCRUM-22's independent checks from the pilot repository root."""

import ast
from pathlib import Path
import sys

sys.path.insert(0, str(Path.cwd()))
from display.greeting import greeting


class PaddedName:
    def __str__(self):
        return "  Ada  "


cases = [
    ("Alice", "Hello, Alice!"),
    ("  Mary Jane  ", "Hello, Mary Jane!"),
    ("", "Hello, friend!"),
    (" \t ", "Hello, friend!"),
    ("  mary  Jane  ", "Hello, mary  Jane!"),
    (None, "Hello, None!"),
    (5, "Hello, 5!"),
    (PaddedName(), "Hello,   Ada  !"),
]

for name, expected in cases:
    actual = greeting(name)
    if actual != expected:
        raise SystemExit(f"greeting({name!r}) returned {actual!r}, expected {expected!r}")

tree = ast.parse(Path("display/greeting.py").read_text(encoding="utf-8"))
if any(
    isinstance(node, ast.Name) and isinstance(node.ctx, ast.Store) and node.id == "name"
    for node in ast.walk(tree)
):
    raise SystemExit("The repair still reassigns the original name parameter")

print(f"{len(cases)} acceptance cases and the parameter-preservation check passed")
