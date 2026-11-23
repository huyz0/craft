use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::Duration;

use crate::order::{WorkOrder, valid_oid};
use crate::{Result, process_tree::read_bounded, reject};

pub(crate) fn git_command() -> Command {
    let mut command = Command::new("git");
    #[cfg(windows)]
    command.args(["-c", "core.longpaths=true"]);
    for (name, _) in std::env::vars_os() {
        if name
            .to_string_lossy()
            .to_ascii_uppercase()
            .starts_with("GIT_")
        {
            command.env_remove(name);
        }
    }
    command
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_NO_REPLACE_OBJECTS", "1");
    command
}

pub(crate) fn disable_external_git_config(command: &mut Command) -> &mut Command {
    // Git for Windows recognizes /dev/null and rejects verbatim paths in GIT_CONFIG_GLOBAL.
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
}

pub(crate) fn isolated_git_command() -> Command {
    let mut command = git_command();
    disable_external_git_config(&mut command);
    command
}

fn git(order: &WorkOrder, args: &[&str]) -> Result<Output> {
    Ok(git_command()
        .arg("-C")
        .arg(&order.repository)
        .args(args)
        .output()?)
}

pub fn prepare_checkout(order: &WorkOrder, destination: &Path, base: &str) -> Result<PathBuf> {
    if !destination.is_absolute() {
        return reject("checkout destination must be absolute");
    }
    let parent = destination
        .parent()
        .ok_or("checkout destination needs a parent directory")?;
    let canonical_parent = parent.canonicalize()?;
    let source = order.repository.canonicalize()?;
    if canonical_parent.starts_with(&source) {
        return reject("checkout destination must be outside the source repository");
    }
    let name = destination
        .file_name()
        .ok_or("checkout destination needs a directory name")?;
    let destination = parent.join(name);
    validate_commit(order, base)?;
    // Reserving a new directory up front prevents Git from reusing an existing checkout.
    std::fs::create_dir(&destination)?;
    let checkout = destination.join("checkout");
    let pack = destination.join("source.pack");
    let mut packing = isolated_git_command()
        .arg("-C")
        .arg(&order.repository)
        .args(["pack-objects", "--revs", "--stdout"])
        .stdin(Stdio::piped())
        .stdout(std::fs::File::create_new(&pack)?)
        .spawn()?;
    let sent = writeln!(packing.stdin.take().ok_or("missing Git input")?, "{base}");
    let status = packing.wait()?;
    sent?;
    if !status.success() {
        return reject("could not export the pinned commit and its reachable objects");
    }
    let format = if base.len() == 64 {
        "--object-format=sha256"
    } else {
        "--object-format=sha1"
    };
    std::fs::create_dir(&checkout)?;
    require_success(
        isolated_git_command()
            .current_dir(&checkout)
            .args(["init", "--quiet", format, "--template="])
            .output()?,
        "could not initialize the isolated checkout",
    )?;
    #[cfg(windows)]
    require_success(
        isolated_git_command()
            .arg("-C")
            .arg(&checkout)
            .args(["config", "--local", "core.longpaths", "true"])
            .output()?,
        "could not enable long paths in the isolated checkout",
    )?;
    require_success(
        isolated_git_command()
            .arg("-C")
            .arg(&checkout)
            .args(["index-pack", "--stdin"])
            .stdin(std::fs::File::open(&pack)?)
            .output()?,
        "could not import the pinned commit pack",
    )?;
    require_success(
        isolated_git_command()
            .arg("-C")
            .arg(&checkout)
            .args([
                "-c",
                "core.hooksPath=.git/factory-disabled-hooks",
                "checkout",
                "--detach",
                base,
            ])
            .output()?,
        "could not check out the pinned commit",
    )?;
    std::fs::remove_file(pack)?;
    Ok(checkout)
}

pub fn prepare_pack_for_verification(
    order: &WorkOrder,
    destination: &Path,
    candidate: &str,
    pack: &[u8],
    repair_head: Option<&str>,
) -> Result<PathBuf> {
    if !valid_oid(candidate) || candidate.len() != order.base.len() || candidate == order.base {
        return reject("candidate must identify a new commit in the pinned repository");
    }
    let checkout = prepare_checkout(order, destination, &order.base)?;
    let pack_path = destination.join("candidate.pack");
    std::fs::File::create_new(&pack_path)?.write_all(pack)?;
    let isolated_git = || {
        let mut command = isolated_git_command();
        command.arg("-C").arg(&checkout);
        command
    };
    require_success(
        isolated_git()
            .args(["index-pack", "--strict", "--stdin"])
            .stdin(std::fs::File::open(&pack_path)?)
            .output()?,
        "candidate pack failed Git object validation",
    )?;
    let mut quarantined = order.clone();
    quarantined.repository = checkout.clone();
    candidate_paths(&quarantined, candidate)?;
    candidate_history_paths(&quarantined, &order.base, candidate)?;
    if let Some(previous) = repair_head {
        if previous == candidate {
            return reject("repair candidate must change the reviewed revision");
        }
        candidate_history_paths(&quarantined, previous, candidate)?;
    }
    require_success(
        isolated_git()
            .args([
                "-c",
                "core.hooksPath=.git/factory-disabled-hooks",
                "checkout",
                "--detach",
                candidate,
            ])
            .output()?,
        "could not check out the scoped candidate",
    )?;
    Ok(checkout)
}

fn require_success(output: Output, reason: &str) -> Result<Output> {
    if output.status.success() {
        Ok(output)
    } else {
        reject(reason)
    }
}

pub fn validate_base(order: &WorkOrder) -> Result<()> {
    let root = require_success(
        git(order, &["rev-parse", "--show-toplevel"])?,
        "cannot resolve the repository root",
    )?;
    require_worktree_root(&order.repository, &root.stdout)?;
    validate_commit(order, &order.base)
}

pub fn validate_ci_head(order: &WorkOrder, head: &str) -> Result<()> {
    validate_commit(order, head).map_err(|error| {
        format!("approved CI branch tip {head} is unavailable in the checkout: {error}")
    })?;
    require_success(
        git(order, &["merge-base", "--is-ancestor", &order.base, head])?,
        "CI branch tip must descend from the approved work order anchor",
    )?;
    Ok(())
}

pub(crate) fn validate_ci_anchor(order: &WorkOrder) -> Result<()> {
    if let Some(anchor) = &order.ci_anchor {
        require_success(
            git(order, &["merge-base", "--is-ancestor", anchor, &order.base])?,
            "CI failure commit must descend from the approved branch anchor",
        )?;
    }
    Ok(())
}

fn require_worktree_root(path: &Path, git_root: &[u8]) -> Result<()> {
    let root = Path::new(std::str::from_utf8(git_root)?.trim_end_matches(['\r', '\n']));
    if root.canonicalize()? != path.canonicalize()? {
        return reject("repository must point to the worktree root");
    }
    Ok(())
}

pub(crate) fn available_worktree(path: &Path) -> bool {
    if !path.is_absolute() || !path.is_dir() {
        return false;
    }
    let mut command = git_command();
    command
        .arg("-C")
        .arg(path)
        .args(["rev-parse", "--show-toplevel"])
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    read_bounded(
        &mut command,
        "Git checkout probe",
        Duration::from_secs(3),
        64 * 1024,
    )
    .is_ok_and(|(status, root)| status.success() && require_worktree_root(path, &root).is_ok())
}

pub(crate) fn validate_verification_checkout(checkout: &Path, expected_head: &str) -> Result<()> {
    if !available_worktree(checkout) {
        return reject("the prepared verification checkout is unavailable");
    }
    let run_git = |args: &[&str]| {
        let mut command = isolated_git_command();
        command
            .arg("-C")
            .arg(checkout)
            .args(["-c", "core.fsmonitor=false"])
            .args(args)
            .stdin(Stdio::null())
            .stderr(Stdio::null());
        read_bounded(
            &mut command,
            "verification checkout check",
            Duration::from_secs(30),
            64 * 1024,
        )
    };
    let (status, head) = run_git(&["rev-parse", "--verify", "HEAD"])?;
    if !status.success() {
        return reject("could not resolve the verification checkout HEAD");
    }
    if std::str::from_utf8(&head)?.trim_end_matches(['\r', '\n']) != expected_head {
        return reject("verification checkout HEAD differs from the prepared candidate");
    }
    let (status, changes) = run_git(&[
        "status",
        "--porcelain=v1",
        "--untracked-files=no",
        "--ignore-submodules=untracked",
    ])?;
    if !status.success() {
        return reject("could not inspect tracked files in the verification checkout");
    }
    if !changes.is_empty() {
        return reject("verification checkout has tracked changes");
    }
    Ok(())
}

pub fn validate_bitbucket_remote(order: &WorkOrder, repository: &str) -> Result<()> {
    validate_remote(order, "bitbucket.org", repository)
}

pub fn validate_github_remote(order: &WorkOrder, repository: &str) -> Result<()> {
    validate_remote(order, "github.com", repository)
}

fn validate_remote(order: &WorkOrder, host: &str, repository: &str) -> Result<()> {
    let remote = require_success(
        git(order, &["remote", "get-url", "--all", "origin"])?,
        "configure an origin for the selected CI repository",
    )?;
    let origin = std::str::from_utf8(&remote.stdout)?.trim();
    let origin = origin.strip_suffix(".git").unwrap_or(origin);
    if ![
        format!("https://{host}/{repository}"),
        format!("git@{host}:{repository}"),
        format!("ssh://git@{host}/{repository}"),
    ]
    .iter()
    .any(|allowed| allowed == origin)
    {
        return reject("the local repository origin does not match the selected CI repository");
    }
    Ok(())
}

fn validate_commit(order: &WorkOrder, oid: &str) -> Result<()> {
    if !valid_oid(oid) {
        return reject("expected a full lowercase commit ID");
    }
    let object = require_success(git(order, &["cat-file", "-t", oid])?, "commit is missing")?;
    if !matches!(object.stdout.as_slice(), b"commit\n" | b"commit\r\n") {
        return reject("the object must be a commit");
    }
    Ok(())
}

pub fn pinned_skill(order: &WorkOrder) -> Result<String> {
    validate_commit(order, &order.skill.revision)?;
    let path = order
        .skill
        .location
        .strip_prefix("repo:")
        .ok_or("skill location must use repo:")?;
    if path.contains([':', '\\'])
        || path.split('/').any(|part| matches!(part, "" | "." | ".."))
        || !path.ends_with("/SKILL.md")
    {
        return reject("skill must point to a repository-relative SKILL.md");
    }
    let object = format!("{}:{path}", order.skill.revision);
    let size = require_success(
        git(order, &["cat-file", "-s", &object])?,
        "pinned skill is missing",
    )?;
    if std::str::from_utf8(&size.stdout)?.trim().parse::<usize>()? > 32 * 1024 {
        return reject("pinned skill exceeds the pilot size limit");
    }
    let skill = require_success(
        git(order, &["cat-file", "blob", &object])?,
        "could not read pinned skill",
    )?;
    Ok(String::from_utf8(skill.stdout)?)
}

pub fn import_candidate(order: &WorkOrder, checkout: &Path, previous: &str) -> Result<String> {
    let mut worker = order.clone();
    worker.repository = checkout.to_owned();
    let tip = require_success(
        git(&worker, &["rev-parse", "--verify", "HEAD"])?,
        "worker checkout has no candidate commit",
    )?;
    let tip = std::str::from_utf8(&tip.stdout)?.trim();
    candidate_paths(&worker, tip)?;
    let parent = require_success(
        git(&worker, &["rev-list", "--parents", "-n", "1", tip])?,
        "could not determine the candidate parent",
    )?;
    let parents: Vec<_> = std::str::from_utf8(&parent.stdout)?
        .split_whitespace()
        .collect();
    if parents.as_slice() != [tip, previous] {
        return reject("worker must create one commit directly on the attempt's pinned head");
    }
    let pack = checkout
        .parent()
        .ok_or("missing worker directory")?
        .join("candidate.pack");
    write_candidate_pack(&worker, tip, previous, &pack)?;
    index_candidate_pack(order, std::fs::File::open(&pack)?.into())?;
    candidate_paths(order, tip)?;
    Ok(tip.to_owned())
}

pub(crate) fn import_interactive_commit(
    order: &WorkOrder,
    checkout: &Path,
    previous: &str,
    candidate: &str,
) -> Result<()> {
    if !available_worktree(checkout) {
        return reject("provide the absolute path to the prepared worktree root");
    }
    let mut worker = order.clone();
    worker.repository = checkout.to_owned();
    candidate_paths(&worker, candidate)?;
    candidate_history_paths(&worker, previous, candidate)?;
    let mut exporter = start_candidate_pack(&worker, candidate, previous, Stdio::piped())?;
    let pack = exporter.stdout.take().ok_or("missing Git candidate pack")?;
    let imported = index_candidate_pack(order, pack.into());
    if !exporter.wait()?.success() {
        return reject("could not export the interactive candidate");
    }
    imported
}

fn index_candidate_pack(order: &WorkOrder, pack: Stdio) -> Result<()> {
    require_success(
        git_command()
            .arg("-C")
            .arg(&order.repository)
            .args(["index-pack", "--strict", "--stdin"])
            .stdin(pack)
            .output()?,
        "could not import the worker commit",
    )?;
    Ok(())
}

pub fn write_candidate_pack(
    order: &WorkOrder,
    head: &str,
    previous: &str,
    pack: &Path,
) -> Result<()> {
    validate_commit(order, head)?;
    validate_commit(order, previous)?;
    let file = std::fs::File::create_new(pack)?;
    let mut exporter = start_candidate_pack(order, head, previous, file.into())?;
    if !exporter.wait()?.success() {
        return reject("could not export the worker commit");
    }
    Ok(())
}

fn start_candidate_pack(
    order: &WorkOrder,
    head: &str,
    previous: &str,
    output: Stdio,
) -> Result<std::process::Child> {
    let mut exporter = git_command()
        .arg("-C")
        .arg(&order.repository)
        .args(["pack-objects", "--revs", "--stdout"])
        .stdin(Stdio::piped())
        .stdout(output)
        .spawn()?;
    let sent = writeln!(
        exporter.stdin.take().ok_or("missing Git input")?,
        "{head}\n^{previous}"
    );
    if let Err(error) = sent {
        let _ = exporter.wait();
        return Err(error.into());
    }
    Ok(exporter)
}

pub fn commit_worker_changes(order: &WorkOrder, checkout: &Path, previous: &str) -> Result<()> {
    let git = |args: &[&str]| -> Result<Output> {
        Ok(isolated_git_command()
            .arg("-C")
            .arg(checkout)
            .args(["-c", "core.hooksPath=.git/factory-disabled-hooks"])
            .args(args)
            .output()?)
    };
    let initial = require_success(
        git(&["rev-parse", "--verify", "HEAD"])?,
        "could not resolve the worker's starting head",
    )?;
    if std::str::from_utf8(&initial.stdout)?.trim() != previous {
        return reject("worker changed Git HEAD instead of leaving edits for the launcher");
    }
    require_success(
        git(&["add", "--all"])?,
        "could not stage changes in the isolated checkout",
    )?;
    let staged = require_success(
        git(&[
            "diff",
            "--cached",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
            "--name-only",
            "-z",
            "--",
        ])?,
        "could not inspect staged worker paths",
    )?;
    validate_paths(order, &staged.stdout)?;
    require_success(
        git(&[
            "-c",
            "user.name=Craft Factory",
            "-c",
            "user.email=factory@example.invalid",
            "-c",
            "commit.gpgSign=false",
            "commit",
            "--quiet",
            "-m",
            "Apply supervised factory candidate",
        ])?,
        "could not create the supervised candidate commit",
    )?;
    Ok(())
}

pub fn candidate_paths(order: &WorkOrder, oid: &str) -> Result<Vec<String>> {
    validate_commit(order, oid)?;
    require_success(
        git(order, &["merge-base", "--is-ancestor", &order.base, oid])?,
        "candidate must descend from the pinned base",
    )?;
    let changes = changed_paths(order, &order.base, oid)?;
    if changes.is_empty() {
        return reject("candidate contains no changes from the pinned base");
    }
    Ok(changes)
}

pub struct DiffFile {
    pub path: String,
    pub lines: Option<(u64, u64)>,
}

pub fn github_enqueue_change_evidence(
    order: &WorkOrder,
    head: &str,
) -> Result<craft_factory_evidence::github_risk::ChangeEvidence> {
    use craft_factory_evidence::github_risk::{ChangeEvidence, MAX_COMMITS, MAX_FILES, MAX_LINES};

    candidate_paths(order, head)?;
    let history = require_success(
        git(
            order,
            &[
                "rev-list",
                "--parents",
                &format!("--max-count={}", MAX_COMMITS + 1),
                &format!("{}..{head}", order.base),
            ],
        )?,
        "could not inspect the proposed enqueue history",
    )?;
    let mut paths = std::collections::BTreeSet::new();
    let mut changed_lines = 0;
    let mut commits = 0;
    let mut file_edits = 0;
    for line in std::str::from_utf8(&history.stdout)?.lines() {
        let mut ancestry = line.split_whitespace();
        let (Some(commit), Some(parent), None) =
            (ancestry.next(), ancestry.next(), ancestry.next())
        else {
            return reject("policy enqueue requires linear history");
        };
        commits += 1;
        if commits > MAX_COMMITS {
            return reject("candidate exceeds the policy commit limit");
        }
        let raw = require_success(
            git(
                order,
                &[
                    "diff-tree",
                    "--no-commit-id",
                    "--no-renames",
                    "--raw",
                    "-r",
                    "-z",
                    parent,
                    commit,
                ],
            )?,
            "could not inspect candidate change types",
        )?;
        if !raw.stdout.is_empty() {
            let mut records = raw
                .stdout
                .strip_suffix(&[0])
                .ok_or("invalid candidate change types")?
                .split(|byte| *byte == 0);
            while let Some(metadata) = records.next() {
                let mut fields = std::str::from_utf8(metadata)?.split_ascii_whitespace();
                let (Some(old), Some(new), Some(_), Some(_), Some("M"), None) = (
                    fields.next(),
                    fields.next(),
                    fields.next(),
                    fields.next(),
                    fields.next(),
                    fields.next(),
                ) else {
                    return reject(
                        "policy enqueue requires modifications to existing regular files",
                    );
                };
                if !matches!(old, ":100644" | ":100755")
                    || old.get(1..) != Some(new)
                    || records.next().is_none()
                {
                    return reject("policy enqueue requires unchanged regular file modes");
                }
            }
        }
        for file in revision_diff(order, parent, commit)? {
            file_edits += 1;
            if file_edits > MAX_FILES {
                return reject("candidate exceeds the policy file-edit limit");
            }
            let (added, removed) = file
                .lines
                .ok_or("policy enqueue cannot classify binary changes")?;
            for revision in [parent, commit] {
                let blob = committed_blob(order, revision, &file.path, 128 * 1024)?;
                if std::str::from_utf8(&blob).is_err() || blob.contains(&0) {
                    return reject("policy enqueue requires UTF-8 text without NUL bytes");
                }
            }
            changed_lines = u32::try_from(
                u64::from(changed_lines)
                    .saturating_add(added)
                    .saturating_add(removed),
            )?;
            paths.insert(file.path);
            if changed_lines > MAX_LINES || paths.len() > MAX_FILES {
                return reject("candidate exceeds the policy diff limits");
            }
        }
    }
    let evidence = ChangeEvidence {
        commits,
        changed_lines,
        paths: paths.into_iter().collect(),
    };
    if !evidence.valid() {
        return reject("candidate does not have a bounded text change history");
    }
    Ok(evidence)
}

pub fn revision_diff(order: &WorkOrder, from: &str, to: &str) -> Result<Vec<DiffFile>> {
    validate_commit(order, from)?;
    validate_commit(order, to)?;
    let diff = require_success(
        git(
            order,
            &[
                "diff-tree",
                "--no-commit-id",
                "--no-renames",
                "--no-ext-diff",
                "--no-textconv",
                "--numstat",
                "-r",
                "-z",
                from,
                to,
            ],
        )?,
        "cannot measure the recorded commit diff",
    )?;
    if diff.stdout.is_empty() {
        return Ok(Vec::new());
    }
    let records = diff
        .stdout
        .strip_suffix(&[0])
        .ok_or("invalid Git diff statistics")?;
    records
        .split(|byte| *byte == 0)
        .map(|record| {
            let fields: Vec<_> = std::str::from_utf8(record)?.splitn(3, '\t').collect();
            let [added, removed, path] = fields.as_slice() else {
                return reject("invalid Git diff statistics");
            };
            if !order.allows(path) {
                return reject("the recorded diff includes an out-of-scope path");
            }
            let lines = if *added == "-" && *removed == "-" {
                None
            } else {
                Some((added.parse()?, removed.parse()?))
            };
            Ok(DiffFile {
                path: (*path).to_owned(),
                lines,
            })
        })
        .collect()
}

pub fn committed_blob(order: &WorkOrder, oid: &str, path: &str, limit: usize) -> Result<Vec<u8>> {
    validate_commit(order, oid)?;
    let object = format!("{oid}:{path}");
    let size = require_success(
        git(order, &["cat-file", "-s", &object])?,
        "committed dependency input is missing",
    )?;
    if std::str::from_utf8(&size.stdout)?.trim().parse::<usize>()? > limit {
        return reject("committed dependency input exceeds the authorized size");
    }
    Ok(require_success(
        git(order, &["cat-file", "blob", &object])?,
        "could not read the committed dependency input",
    )?
    .stdout)
}

pub fn candidate_history_paths(
    order: &WorkOrder,
    previous: &str,
    candidate: &str,
) -> Result<std::collections::BTreeSet<String>> {
    validate_commit(order, previous)?;
    validate_commit(order, candidate)?;
    require_success(
        git(order, &["merge-base", "--is-ancestor", previous, candidate])?,
        "candidate must descend from the expected prior head",
    )?;
    let range = format!("{previous}..{candidate}");
    let history = require_success(
        git(order, &["rev-list", "--parents", &range])?,
        "could not inspect the candidate commit history",
    )?;
    let mut paths = std::collections::BTreeSet::new();
    for line in std::str::from_utf8(&history.stdout)?.lines() {
        let commits: Vec<_> = line.split_whitespace().collect();
        let [commit, parent] = commits.as_slice() else {
            return reject("submit a linear candidate history without merge commits");
        };
        paths.extend(changed_paths(order, parent, commit)?);
    }
    Ok(paths)
}

fn changed_paths(order: &WorkOrder, from: &str, to: &str) -> Result<Vec<String>> {
    let changes = require_success(
        git(
            order,
            &[
                "diff-tree",
                "--no-commit-id",
                "--no-renames",
                "--name-only",
                "-r",
                "-z",
                from,
                to,
            ],
        )?,
        "cannot inspect candidate changes",
    )?;
    if changes.stdout.is_empty() {
        Ok(Vec::new())
    } else {
        validate_paths(order, &changes.stdout)
    }
}

pub fn retain_candidate(order: &WorkOrder, oid: &str) -> Result<()> {
    let reference = format!("refs/craft-factory/candidates/{}/{oid}", order.id);
    let hooks = if cfg!(windows) {
        "core.hooksPath=NUL"
    } else {
        "core.hooksPath=/dev/null"
    };
    require_success(
        git(
            order,
            &[
                "-c",
                hooks,
                "-c",
                "core.longpaths=true",
                "update-ref",
                "--no-deref",
                &reference,
                oid,
            ],
        )?,
        "could not retain the scoped candidate in the local repository",
    )?;
    Ok(())
}

pub fn verify_retry_head(order: &WorkOrder, previous: &str, next: &str) -> Result<()> {
    validate_commit(order, previous)?;
    validate_commit(order, next)?;
    let before = format!("{previous}^{{tree}}");
    let after = format!("{next}^{{tree}}");
    let parents = require_success(
        git(order, &["rev-list", "--parents", "-n", "1", next])?,
        "could not read the retry commit's parents",
    )?;
    if !std::str::from_utf8(&parents.stdout)?
        .split_whitespace()
        .eq([next, previous])
    {
        return reject("a CI retry head must directly descend from the current candidate");
    }
    let trees = require_success(
        git(order, &["rev-parse", &before, &after])?,
        "could not resolve retry commit trees",
    )?;
    let mut trees = std::str::from_utf8(&trees.stdout)?.split_whitespace();
    if trees
        .next()
        .zip(trees.next())
        .is_none_or(|(base, candidate)| base != candidate)
    {
        return reject("a CI retry head must preserve the candidate's exact file tree");
    }
    Ok(())
}

fn validate_paths(order: &WorkOrder, changes: &[u8]) -> Result<Vec<String>> {
    let mut paths = Vec::new();
    if changes.is_empty() {
        return reject("candidate contains no changes from the pinned base");
    }
    let list = changes.strip_suffix(&[0]).ok_or("invalid Git path list")?;
    for path in list.split(|byte| *byte == 0) {
        let path = std::str::from_utf8(path)?;
        if !order.allows(path) {
            return reject(format!("candidate changes an out-of-scope path: {path:?}"));
        }
        paths.push(path.to_owned());
    }
    Ok(paths)
}

#[cfg(test)]
mod enqueue_risk_tests {
    use super::*;

    #[test]
    fn policy_counts_intermediate_edits_and_disallows_reverted_additions() {
        let root = std::env::temp_dir().join(format!(
            "craft-risk-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let run = |args: &[&str]| {
            let output = git_command()
                .arg("-C")
                .arg(&root)
                .args([
                    "-c",
                    "user.name=Fixture",
                    "-c",
                    "user.email=fixture@example.invalid",
                ])
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap().trim().to_owned()
        };
        run(&["init", "-q"]);
        std::fs::create_dir(root.join("src")).unwrap();
        std::fs::write(root.join("src/story"), "base\n").unwrap();
        std::fs::write(root.join("src/transient"), "original\n").unwrap();
        run(&["add", "--", "src"]);
        run(&["commit", "-qm", "base"]);
        let base = run(&["rev-parse", "HEAD"]);
        let order: WorkOrder = serde_json::from_value(serde_json::json!({
            "schema_version":1,"id":"risk","issue":"RISK-1","owner":"fixture","repository":root,
            "base":base,"intent":"fixture","acceptance":["fixture"],"scope":["src"],
            "context":[],"skill":{"location":"fixture","revision":base}
        }))
        .unwrap();
        std::fs::write(root.join("src/story"), "final\n").unwrap();
        std::fs::write(root.join("src/transient"), "intermediate\n").unwrap();
        run(&["commit", "-qam", "changes"]);
        std::fs::write(root.join("src/transient"), "original\n").unwrap();
        run(&["commit", "-qam", "revert one edit"]);
        let head = run(&["rev-parse", "HEAD"]);
        let measured = github_enqueue_change_evidence(&order, &head).unwrap();
        assert_eq!(measured.commits, 2);
        assert_eq!(measured.changed_lines, 6);
        assert_eq!(measured.paths, ["src/story", "src/transient"]);
        std::fs::write(root.join("src/unapproved"), "temporarily added\n").unwrap();
        run(&["add", "--", "src/unapproved"]);
        run(&["commit", "-qm", "add file"]);
        run(&["rm", "-q", "--", "src/unapproved"]);
        run(&["commit", "-qm", "remove file"]);
        let unchanged_net = run(&["rev-parse", "HEAD"]);
        assert!(github_enqueue_change_evidence(&order, &unchanged_net).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
