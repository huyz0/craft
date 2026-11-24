use std::path::{Path, PathBuf};
use std::time::Duration;

use craft_factory_evidence::{release_trace::read_evidence, sha256};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::order::{CiFailure, WorkOrder};
use crate::store::{CiAttempt, CiDispatch, CiPollBranch, Store};
use crate::{Result, github, read_json, reject, repository, runner, twg};

#[derive(clap::Args)]
pub struct Options {
    #[arg(
        long,
        required_unless_present = "bitbucket_policy",
        conflicts_with = "bitbucket_policy",
        requires = "gh"
    )]
    github_policy: Option<PathBuf>,
    #[arg(long, requires = "github_policy")]
    gh: Option<PathBuf>,
    #[arg(long)]
    bitbucket_policy: Option<PathBuf>,
    /// Existing private directory for isolated attempt checkouts.
    #[arg(long)]
    attempt_root: PathBuf,
    #[command(flatten)]
    adapter: runner::AdapterOptions,
    #[arg(long, default_value_t = 900, value_parser = clap::value_parser!(u64).range(1..=3600))]
    timeout_seconds: u64,
}

#[derive(clap::Args)]
pub struct RepairOptions {
    #[command(flatten)]
    dispatch: Options,
    /// Explicitly retry this scheduled work order after an interrupted or failed attempt.
    #[arg(long, value_name = "WORK_ID")]
    retry: Option<String>,
}

#[derive(clap::Args)]
pub struct CycleOptions {
    /// Work order template without a CI observation.
    order: PathBuf,
    #[command(flatten)]
    dispatch: Options,
    #[arg(
        long,
        requires = "bitbucket_policy",
        required_unless_present = "github_policy"
    )]
    step_name: Option<String>,
    #[arg(long)]
    jira_snapshot: Option<PathBuf>,
    /// Exact reviewed work order SHA-256, with the sha256: prefix.
    #[arg(long, requires = "policy_digest")]
    order_digest: Option<String>,
    /// Exact reviewed forge policy SHA-256, with the sha256: prefix.
    #[arg(long, requires = "order_digest")]
    policy_digest: Option<String>,
    /// Replace a local JSON report after a completed cycle.
    #[arg(long)]
    report_file: Option<PathBuf>,
    /// Follow the reviewed policy branch while retaining the approved order's anchor.
    #[arg(long, requires_all = ["order_digest", "policy_digest"])]
    follow_branch: bool,
    /// Admit and dispatch one repair. Otherwise only record a shadow decision.
    #[arg(long)]
    admit: bool,
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..=10))]
    daily_limit: u32,
}

pub enum PollProvider<'a> {
    Github {
        policy: &'a Path,
        gh: &'a Path,
    },
    Bitbucket {
        policy: &'a Path,
        step_name: &'a str,
    },
}

pub(crate) enum Provider {
    Github {
        policy: github::Policy,
        gh: PathBuf,
        digest: String,
    },
    Bitbucket(twg::Policy),
}

impl Provider {
    fn load(options: &Options, digest: Option<&str>) -> Result<Self> {
        match (
            options.github_policy.as_deref(),
            &options.gh,
            options.bitbucket_policy.as_deref(),
        ) {
            (Some(policy), Some(gh), None) => Self::load_github(policy, gh, digest),
            (None, None, Some(policy)) => {
                Ok(Self::Bitbucket(read_input(policy, digest, "CI policy")?))
            }
            _ => reject("configure the provider of the admitted CI repair"),
        }
    }

    fn load_github(path: &Path, gh: &Path, pin: Option<&str>) -> Result<Self> {
        let (policy, digest) = github::read_pinned_policy(path)?;
        if pin.is_some_and(|pin| pin != digest) {
            return reject("CI policy does not match the approved digest");
        }
        Ok(Self::Github {
            policy,
            gh: gh.to_owned(),
            digest,
        })
    }

    fn repository(&self) -> String {
        match self {
            Self::Github { policy, .. } => format!("github.com/{}", policy.repository),
            Self::Bitbucket(policy) => policy.repository_name(),
        }
    }

    pub(crate) fn branch(&self) -> &str {
        match self {
            Self::Github { policy, .. } => &policy.base_branch,
            Self::Bitbucket(policy) => &policy.base_branch,
        }
    }

    pub(crate) fn validate_repository(&self, order: &WorkOrder) -> Result<()> {
        match self {
            Self::Github { policy, .. } => {
                repository::validate_github_remote(order, &policy.repository)
            }
            Self::Bitbucket(policy) => {
                repository::validate_bitbucket_remote(order, &policy.repository_name())
            }
        }
    }

    pub(crate) fn branch_tip(&self) -> Result<String> {
        match self {
            Self::Github { policy, gh, .. } => github::ci_branch_tip(policy, gh),
            Self::Bitbucket(policy) => twg::ci_branch_tip(policy),
        }
    }

    pub(crate) fn observe(
        &self,
        order: &WorkOrder,
        step_name: Option<&str>,
    ) -> Result<Option<CiFailure>> {
        match self {
            Self::Github { policy, gh, .. } if step_name.is_none() => {
                Ok(github::poll_failed_job(policy, gh, &order.base)?.map(CiFailure::Github))
            }
            Self::Bitbucket(policy) => {
                let step = step_name.ok_or("supply --step-name for Bitbucket")?;
                Ok(twg::poll_failed_step(policy, step, &order.base)?.map(CiFailure::Bitbucket))
            }
            _ => reject("configure one provider and supply --step-name only for Bitbucket"),
        }
    }

    fn recheck(&self, order: &WorkOrder) -> Result<()> {
        match (&order.ci_failure, self) {
            (Some(CiFailure::Github(original)), Self::Github { policy, gh, digest }) => {
                if order.ci_verification_policy_digest.is_some()
                    && order.ci_policy_digest.as_deref() != Some(digest)
                {
                    return reject("GitHub policy differs from the admitted policy pin");
                }
                repository::validate_github_remote(order, &policy.repository)?;
                if github::failed_job(
                    policy,
                    gh,
                    original.run_id,
                    original.run_attempt,
                    &order.base,
                )? != *original
                {
                    return reject(
                        "the admitted GitHub failure changed; review the repair intent again",
                    );
                }
            }
            (Some(CiFailure::Bitbucket(original)), Self::Bitbucket(policy)) => {
                repository::validate_bitbucket_remote(order, &policy.repository_name())?;
                if twg::failed_step(policy, original.build, &original.step_uuid, &order.base)?
                    != *original
                {
                    return reject(
                        "the admitted Bitbucket failure changed; review the repair intent again",
                    );
                }
            }
            _ => return reject("configure the provider of the admitted CI repair"),
        }
        Ok(())
    }
}

fn read_input<T: DeserializeOwned>(path: &Path, digest: Option<&str>, label: &str) -> Result<T> {
    let Some(digest) = digest else {
        return read_json(path);
    };
    let bytes = read_evidence(path, 128 * 1024).map_err(|error| error.to_string())?;
    if sha256(&bytes) != digest {
        return reject(format!("{label} does not match the approved digest"));
    }
    Ok(serde_json::from_slice(&bytes)?)
}

pub fn poll(
    state: &Path,
    order: &Path,
    provider: PollProvider<'_>,
    jira_snapshot: Option<&Path>,
    admit: bool,
    daily_limit: u32,
) -> Result<Value> {
    let order = read_json(order)?;
    let (provider, step_name) = match provider {
        PollProvider::Github { policy, gh } => (Provider::load_github(policy, gh, None)?, None),
        PollProvider::Bitbucket { policy, step_name } => {
            (Provider::Bitbucket(read_json(policy)?), Some(step_name))
        }
    };
    poll_order(
        state,
        order,
        &provider,
        PollOptions {
            step_name,
            jira_snapshot,
            admit,
            daily_limit,
            follow_branch: false,
        },
    )
}

struct PollOptions<'a> {
    step_name: Option<&'a str>,
    jira_snapshot: Option<&'a Path>,
    admit: bool,
    daily_limit: u32,
    follow_branch: bool,
}

fn poll_order(
    state: &Path,
    mut order: WorkOrder,
    provider: &Provider,
    options: PollOptions<'_>,
) -> Result<Value> {
    let mut branch = options.follow_branch.then(|| CiPollBranch {
        name: provider.branch(),
        anchor: order.base.clone(),
        observed_head: None,
    });
    let evidence = (|| {
        if order.ci_failure.is_some() {
            return reject("provide a work order template without CI observations");
        }
        order.validate()?;
        if order.ci_verification_policy_digest.is_some()
            && !matches!(provider, Provider::Github { .. })
        {
            return reject("pinned pre-draft verification is supported for local GitHub CI repair");
        }
        repository::validate_base(&order)?;
        provider.validate_repository(&order)?;
        if let Some(branch) = &mut branch {
            if order.dependency_intake_digest.is_some() {
                return reject("a dependency update must keep its exact approved commit");
            }
            let head = provider.branch_tip()?;
            branch.observed_head = Some(head.clone());
            repository::validate_ci_head(&order, &head)?;
            order.base = head;
        }
        let Some(failure) = provider.observe(&order, options.step_name)? else {
            return Ok(None);
        };
        let digest = failure.poll_digest()?;
        order.id = format!("{}-ci-{digest}", &order.id[..order.id.len().min(32)]);
        order.require_native_checks = true;
        order.ci_failure = Some(failure);
        if order.ci_verification_policy_digest.is_some()
            && let Provider::Github { digest, .. } = provider
        {
            order.ci_policy_digest = Some(digest.clone());
        }
        Ok(Some(order))
    })();
    Store::open(state)?.record_ci_poll(
        &provider.repository(),
        evidence,
        options.jira_snapshot,
        options.admit,
        options.daily_limit,
        branch,
    )
}

pub fn cycle(state: &Path, options: CycleOptions) -> Result<Value> {
    let report_path = options
        .report_file
        .as_deref()
        .map(|path| report_destination(path, state))
        .transpose()?;
    let order = read_input(
        &options.order,
        options.order_digest.as_deref(),
        "CI work order",
    )?;
    let provider = Provider::load(&options.dispatch, options.policy_digest.as_deref())?;
    let receipt = poll_order(
        state,
        order,
        &provider,
        PollOptions {
            step_name: options.step_name.as_deref(),
            jira_snapshot: options.jira_snapshot.as_deref(),
            admit: options.admit,
            daily_limit: options.daily_limit,
            follow_branch: options.follow_branch,
        },
    )?;
    let dispatch = match (options.admit, receipt["decision"].as_str()) {
        (true, Some("admitted" | "already_admitted")) => {
            let id = receipt["work_order_id"]
                .as_str()
                .ok_or("missing admitted work order ID")?;
            Some(
                dispatch_one(
                    state,
                    &options.dispatch,
                    Some(id),
                    CiAttempt::Initial,
                    |order| provider.recheck(order),
                )
                .unwrap_or_else(|error| reply("blocked", Some(id), None, Some(&error.to_string()))),
            )
        }
        _ => None,
    };
    let mut report = json!({ "schema_version": 1, "poll": receipt, "dispatch": dispatch });
    if let (Some(order), Some(policy)) = (options.order_digest, options.policy_digest) {
        report["input_digests"] = json!({"work_order": order, "policy": policy});
    }
    if let Some(path) = report_path {
        require_cycle_report(&path)?;
        report["kind"] = json!("ci_cycle_report");
        report["completed_at"] = json!(crate::evidence_capture::now()?);
        crate::evidence_capture::replace_json(&path, &report)?;
    }
    Ok(report)
}

fn report_destination(path: &Path, state: &Path) -> Result<PathBuf> {
    fn location(path: &Path) -> Result<PathBuf> {
        let filename = path
            .file_name()
            .ok_or("report and state require file paths")?;
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        Ok(parent.canonicalize()?.join(filename))
    }

    let path = location(path)?;
    let state = location(state)?;
    let state_files = ["-wal", "-shm", "-journal"].map(|suffix| {
        let mut name = state.as_os_str().to_owned();
        name.push(suffix);
        PathBuf::from(name)
    });
    let matches = |other: &Path| {
        if cfg!(windows) {
            path.to_string_lossy()
                .eq_ignore_ascii_case(&other.to_string_lossy())
        } else {
            path == other
        }
    };
    if matches(&state) || state_files.iter().any(|file| matches(file)) {
        return reject("the CI report must be separate from the state database");
    }
    require_cycle_report(&path)?;
    Ok(path)
}

fn require_cycle_report(path: &Path) -> Result<()> {
    match path.symlink_metadata() {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
        Ok(metadata) if metadata.is_file() && metadata.len() <= 512 * 1024 => {
            let bytes = read_evidence(path, 512 * 1024).map_err(|error| error.to_string())?;
            if serde_json::from_slice::<Value>(&bytes)
                .is_ok_and(|value| value["kind"] == "ci_cycle_report")
            {
                return Ok(());
            }
        }
        Ok(_) => (),
    }
    reject("the CI report path already exists and is not a CI cycle report")
}

pub fn run(state: &Path, options: RepairOptions) -> Result<Value> {
    let attempt = if options.retry.is_some() {
        CiAttempt::Retry
    } else {
        CiAttempt::Initial
    };
    dispatch_one(
        state,
        &options.dispatch,
        options.retry.as_deref(),
        attempt,
        |order| Provider::load(&options.dispatch, None)?.recheck(order),
    )
}

fn dispatch_one(
    state: &Path,
    options: &Options,
    expected_id: Option<&str>,
    attempt: CiAttempt,
    recheck: impl FnOnce(&WorkOrder) -> Result<()>,
) -> Result<Value> {
    let mut store = Store::open(state)?;
    let dispatch = store.ci_dispatch()?;
    let id = match &dispatch {
        CiDispatch::Paused => return Ok(reply("paused", None, None, None)),
        CiDispatch::Idle => None,
        CiDispatch::Ready(id) | CiDispatch::NeedsOperator(id) => Some(id.as_str()),
    };
    if matches!(attempt, CiAttempt::Retry)
        && !expected_id.is_some_and(|id| attempt.can_start(&dispatch, id))
    {
        return Ok(reply(
            "blocked",
            expected_id,
            None,
            Some("the named scheduled repair is not waiting for a retry"),
        ));
    }
    let Some(id) = id.filter(|id| expected_id.is_none_or(|expected| expected == *id)) else {
        return Ok(reply("idle", None, None, None));
    };
    if !attempt.can_start(&dispatch, id) {
        return Ok(reply("needs_operator", Some(id), None, None));
    }
    let mut nonce = [0_u8; 8];
    getrandom::getrandom(&mut nonce)
        .map_err(|_| "could not generate the attempt directory name")?;
    if !options.attempt_root.is_absolute() || !options.attempt_root.is_dir() {
        return reject("an absolute, existing attempt root is required");
    }
    let destination = options
        .attempt_root
        .join(format!("ci-{:016x}", u64::from_be_bytes(nonce)));
    let runtime = options.adapter.resolve()?;
    let result = runner::run_ci(
        state,
        id,
        runner::Options {
            destination,
            runtime,
            timeout: Duration::from_secs(options.timeout_seconds),
            shared_repair: None,
            team_skill: None,
            project_context: Vec::new(),
            collect_knowledge: false,
        },
        attempt,
        recheck,
    );
    match result {
        Ok(receipt) => Ok(reply(
            "candidate_ready",
            Some(id),
            receipt.candidate.as_deref(),
            None,
        )),
        Err(error) => Ok(reply("blocked", Some(id), None, Some(&error.to_string()))),
    }
}

fn reply(decision: &str, id: Option<&str>, candidate: Option<&str>, reason: Option<&str>) -> Value {
    json!({ "schema_version": 1, "decision": decision, "work_order_id": id,
        "candidate": candidate, "reason": reason })
}
