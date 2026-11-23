"""Select and validate the credential-free Factory CI lane."""

import argparse
import json
import os
from pathlib import Path
import re
import subprocess
from urllib.parse import unquote, urlsplit


DOCS = {
    'factory/README.md',
    'factory/QUICKSTART.md',
    'factory/QUICKSTART-LINUX.md',
    'factory/deploy/systemd/README.md',
    'factory/deploy/windows/README.md',
    'factory/pilot-handoff.md',
    'factory/shared/README.md',
    'docs/plans/factory-delivery-plan.md',
    'docs/researches/factory-architecture-and-stack.md',
    'docs/researches/factory-mvp-and-end-state.md',
    'docs/researches/factory-requirements-and-pilot.md',
}

REGULAR_EDIT = re.compile(rb':100644 100644 [0-9a-f]+ [0-9a-f]+ M')
LINK = re.compile(r'\]\((?:<([^>]+)>|([^\s)]+))(?:\s+"[^"]*")?\)')
HEADING = re.compile(r'^ {0,3}#{1,6}\s+(.+?)\s*#*\s*$')
FENCE = re.compile(r'^ {0,3}(`{3,}|~{3,})')


def git(root, *args):
    return subprocess.run(['git', *args], cwd=root, check=True, capture_output=True).stdout


def docs_only_changes(diff):
    if not diff.endswith(b'\0'):
        return False
    fields = diff[:-1].split(b'\0')
    return len(fields) % 2 == 0 and all(
        REGULAR_EDIT.fullmatch(metadata) and os.fsdecode(path) in DOCS
        for metadata, path in zip(fields[::2], fields[1::2])
    )


def select(root, event, base, head):
    if event != 'pull_request' or not base or not head:
        return 'full', 'This event requires the full suite.'
    try:
        parents = git(root, 'show', '-s', '--format=%P', 'HEAD').decode('ascii').split()
        if parents != [base, head]:
            return 'full', 'The checkout does not match the pull request merge parents.'
        changes = git(root, 'diff', '--raw', '--no-renames', '-z', 'HEAD^1', 'HEAD', '--')
    except (OSError, ValueError, subprocess.CalledProcessError):
        return 'full', 'The pull request comparison was unavailable.'
    if docs_only_changes(changes):
        return 'docs', 'Only approved Factory documentation files were edited.'
    return 'full', 'The change is empty or includes files outside the documentation shortcut.'


def markdown_lines(path):
    fence = None
    for number, line in enumerate(path.read_text(encoding='utf-8').splitlines(), 1):
        marker = FENCE.match(line)
        if marker:
            token = marker[1]
            if fence is None:
                fence = token
            elif token.startswith(fence) and not line[marker.end():].strip():
                fence = None
        elif fence is None:
            yield number, line


def headings(path):
    seen = set()
    for _, line in markdown_lines(path):
        if match := HEADING.fullmatch(line):
            plain = re.sub(r'\[([^]]+)\]\([^)]+\)', r'\1', match[1]).lower()
            slug = ''.join(char for char in plain if char.isalnum() or char in ' _-').replace(' ', '-')
            unique = slug
            count = 1
            while unique in seen:
                unique = f'{slug}-{count}'
                count += 1
            seen.add(unique)
    return seen


def validate_docs(root, documents=None):
    errors = []
    for name in sorted(DOCS if documents is None else documents):
        source = root / name
        for number, line in markdown_lines(source):
            for link in LINK.finditer(line):
                address = link[1] or link[2]
                parsed = urlsplit(address)
                if parsed.scheme or parsed.netloc:
                    continue
                target = ((source.parent / unquote(parsed.path)) if parsed.path else source).resolve()
                if not target.is_relative_to(root) or not target.exists():
                    errors.append(f'{name}:{number}: invalid local target {address}')
                elif target.suffix == '.md' and parsed.fragment and unquote(parsed.fragment) not in headings(target):
                    errors.append(f'{name}:{number}: missing section {address}')
                elif target.suffix == '.json':
                    try:
                        json.loads(target.read_text(encoding='utf-8'))
                    except (OSError, ValueError) as error:
                        errors.append(f'{name}:{number}: invalid JSON example {address}: {error}')
    return errors


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=('select', 'validate-docs'))
    parser.add_argument('--kit', type=Path, help='validate documentation in an extracted launcher kit')
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    if args.command == 'validate-docs':
        if args.kit:
            root = args.kit.resolve(strict=True)
            documents = [path.relative_to(root).as_posix() for path in root.rglob('*.md')]
            if not {'QUICKSTART.md', 'README.md'}.issubset(documents):
                raise SystemExit('Extracted kit is missing its setup or workflow guide.')
            errors = validate_docs(root, documents)
        else:
            errors = validate_docs(root)
        if errors:
            raise SystemExit('\n'.join(errors))
        print('Factory documentation links, Markdown sections, and linked JSON examples passed.')
        return
    if args.kit:
        parser.error('--kit is only available with validate-docs')
    lane, reason = select(root, os.getenv('GITHUB_EVENT_NAME'), os.getenv('PR_BASE'), os.getenv('PR_HEAD'))
    with open(os.environ['GITHUB_OUTPUT'], 'a', encoding='utf-8') as output:
        output.write(f'lane={lane}\n')
    with open(os.environ['GITHUB_STEP_SUMMARY'], 'a', encoding='utf-8') as summary:
        summary.write(f'Factory CI lane: **{lane}**. {reason}\n')
    print(reason)


if __name__ == '__main__':
    main()
