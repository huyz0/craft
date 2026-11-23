#!/usr/bin/env bash
set -euo pipefail

if [[ $# != 4 && $# != 5 ]]; then
  printf 'Usage: %s EXTRACTED_KIT CODEX_ARCHIVE CLAUDE_ARCHIVE POSTGRES_BIN [COMPARISON_REPORT]\n' "$0" >&2
  exit 2
fi

tools=$(dirname -- "$(realpath -e -- "$0")")
source=$(dirname -- "$tools")
fixtures="$source/tests/fixtures"
kit=$(realpath -e -- "$1")
codex_archive=$(realpath -e -- "$2")
claude_archive=$(realpath -e -- "$3")
postgres_bin=$(realpath -e -- "$4")

printf '%s  %s\n' '5c9cf121bedf9def3be41ec0b903cea014b4165b667350aa12e75995de3e8514204a3a1f9a959d35db1f6c58407533f89bfd348a5edd7f9738d8cab239f183c2' "$codex_archive" 'f469fe57b0ed27c3359f3b4ff42783565f60dd1b6b7a923bace153c8f4519d97bc0facde6c81ca982ecebc16aeb9cb803f45f493ce520cdfcb3d56728ec19ecd' "$claude_archive" | sha512sum --check

scratch=$(mktemp -d /tmp/craft-native-release.XXXXXXXX)
trap 'rm -rf -- "$scratch"' EXIT
mkdir "$scratch/codex" "$scratch/claude"
cp -a -- "$kit" "$scratch/kit"
tar -xzf "$codex_archive" -C "$scratch/codex" package/vendor
tar -xzf "$claude_archive" -C "$scratch/claude" package/claude
codex="$scratch/codex/package/vendor/x86_64-unknown-linux-musl/bin/codex"
claude="$scratch/claude/package/claude"
bubblewrap=$(command -v bwrap)
rust_toolchain=$(python3 -c 'import sys, tomllib; print(tomllib.load(open(sys.argv[1], "rb"))["toolchain"]["channel"])' \
  "$source/rust-toolchain.toml")
report_args=()
if [[ $# == 5 ]]; then
  report_args=(--comparison-report "$scratch/comparison.json")
fi

run_isolated() {
  "$bubblewrap" --unshare-net --unshare-pid --ro-bind / / --proc /proc --dev /dev --tmpfs /tmp \
    --bind "$scratch" "$scratch" --ro-bind "$tools" "$tools" --ro-bind "$fixtures" "$fixtures" \
    --ro-bind "$postgres_bin" "$postgres_bin" --chdir "$source" \
    --clearenv --setenv HOME "$scratch" --setenv TMPDIR "$scratch" \
    --setenv PATH "$PATH" --setenv LANG C.UTF-8 --setenv LD_LIBRARY_PATH "${LD_LIBRARY_PATH:-}" \
    --setenv RUSTUP_HOME "${RUSTUP_HOME:-$HOME/.rustup}" --setenv RUSTUP_TOOLCHAIN "$rust_toolchain" -- "$@"
}

run_isolated \
  python3 -B "$tools/qualify_native_claude_worker.py" "$scratch/kit/craft-factory" "$claude" "$bubblewrap" \
    --shared-kit "$scratch/kit" --postgres-bin "$postgres_bin" --codex "$codex" "${report_args[@]}"

for tracker in --github-issue --jira-issue; do
  run_isolated \
    python3 -B "$tools/qualify_native_shared_worker.py" "$scratch/kit" "$codex" \
      --postgres-bin "$postgres_bin" "$tracker"
done

if [[ $# == 5 ]]; then
  (set -o noclobber; cat "$scratch/comparison.json" > "$5")
fi
