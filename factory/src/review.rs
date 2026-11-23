use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::order::{PinnedReference, valid_oid};
use crate::store::Receipt;
use crate::{Result, reject};

pub fn read_feedback_summary(path: &std::path::Path) -> Result<String> {
    use std::io::Read;
    const MAX_SUMMARY: u64 = 8 * 1024;
    let mut note = String::new();
    std::fs::File::open(path)?
        .take(MAX_SUMMARY + 1)
        .read_to_string(&mut note)?;
    if note.len() as u64 > MAX_SUMMARY || note.trim().is_empty() {
        return reject("provide a nonempty review summary up to 8 KiB");
    }
    Ok(note.trim().to_owned())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Draft {
    pub reference: String,
    pub head: String,
}

#[derive(Deserialize)]
pub(crate) struct FeedbackTarget {
    pub draft: Draft,
    pub base: String,
    pub issue: Option<String>,
}

pub(crate) fn normalize_repair_note(body: &str, provider: &str) -> Result<String> {
    if body
        .chars()
        .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return reject(format!(
            "the {provider} review note contains invalid control characters"
        ));
    }
    let summary = body.split_whitespace().collect::<Vec<_>>().join(" ");
    if summary.is_empty() || summary.chars().count() > 1000 {
        return reject(format!(
            "provide a {provider} review note with 1 to 1000 characters after whitespace normalization"
        ));
    }
    Ok(summary)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReviewRecord {
    pub schema_version: u32,
    pub draft: Draft,
    #[serde(default)]
    pub source: EvidenceSource,
    pub actor: String,
    pub evidence: String,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MergeObservation {
    pub schema_version: u32,
    pub provider: String,
    pub draft: Draft,
    pub pinned_base: String,
    pub destination_branch: String,
    pub result_commit: String,
    pub merged_at: i64,
    pub actor: String,
    pub reference: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub approvals: Vec<MergeApproval>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MergeApproval {
    pub actor: String,
    pub approved_at: i64,
    pub reference: String,
}

impl MergeObservation {
    pub fn review_record(&self) -> ReviewRecord {
        ReviewRecord {
            schema_version: 1,
            draft: self.draft.clone(),
            source: EvidenceSource::OperatorObserved,
            actor: self.actor.clone(),
            evidence: self.reference.clone(),
            note: format!(
                "Observed merge into {} as {}.",
                self.destination_branch, self.result_commit
            ),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VerificationHandoff {
    pub schema_version: u32,
    pub work_order_id: String,
    pub draft: Draft,
    pub base: String,
    pub issue: String,
    pub intent: String,
    pub acceptance: Vec<String>,
    pub scope: Vec<String>,
    pub changed_paths: Vec<String>,
    pub context: Vec<PinnedReference>,
    pub skill: PinnedReference,
    pub native_check_sequence: Option<i64>,
    pub native_evidence: Option<serde_json::Value>,
}

impl VerificationHandoff {
    pub fn digest(&self) -> Result<String> {
        Ok(craft_factory_evidence::sha256(&serde_json::to_vec(self)?))
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceSource {
    #[default]
    Unspecified,
    OperatorObserved,
    Simulated,
}

pub fn merge_approval_matches(
    receipt: &Receipt,
    observation_kind: &str,
    configured: Option<&str>,
) -> Result<()> {
    let recorded = receipt
        .events
        .iter()
        .rev()
        .find(|event| event.kind == observation_kind && event.candidate == receipt.candidate)
        .map(|event| serde_json::from_str::<serde_json::Value>(&event.note))
        .transpose()?;
    if recorded
        .as_ref()
        .and_then(|note| note["merge_approval_policy"].as_str())
        != configured
    {
        return reject(
            "the merge approval policy differs from the one recorded during candidate verification",
        );
    }
    Ok(())
}

impl Draft {
    pub fn validate(&self) -> Result<()> {
        if !valid_oid(&self.head) || self.reference.trim().is_empty() {
            return reject("a draft reference and full candidate commit ID are required");
        }
        Ok(())
    }
}

impl ReviewRecord {
    pub fn validate(&self) -> Result<()> {
        self.draft.validate()?;
        if self.schema_version != 1 {
            return reject("unsupported review record version");
        }
        if [&self.actor, &self.evidence, &self.note]
            .iter()
            .any(|item| item.trim().is_empty())
        {
            return reject("actor, evidence reference and note are required");
        }
        Ok(())
    }
}

pub fn write_candidate_summary(
    out: &mut impl std::io::Write,
    order: &crate::order::WorkOrder,
    head: &str,
) -> Result<()> {
    let changes = crate::repository::revision_diff(order, &order.base, head)?;
    writeln!(
        out,
        "## Candidate for {}\n\nRevision `{head}` against `{}`\n",
        markdown(&order.issue, 100),
        order.base,
    )?;
    writeln!(out, "### Intent\n\n{}\n", markdown(&order.intent, 800))?;
    match &order.ci_failure {
        Some(crate::order::CiFailure::Bitbucket(failure)) => writeln!(
            out,
            "Bitbucket build {} failed in **{}** on `{}`. [Failed step]({})\n",
            failure.build,
            markdown(&failure.step_name, 200),
            failure.commit,
            failure.evidence_url,
        )?,
        Some(crate::order::CiFailure::Github(failure)) => writeln!(
            out,
            "GitHub Actions run {} attempt {} failed in **{}** on `{}`. [Failed job]({})\n",
            failure.run_id,
            failure.run_attempt,
            markdown(&failure.job_name, 200),
            failure.commit,
            failure.evidence_url,
        )?,
        None => (),
    }
    writeln!(out, "### Test intent\n")?;
    for item in order.acceptance.iter().take(8) {
        writeln!(out, "- {}", markdown(item, 300))?;
    }
    if order.acceptance.len() > 8 {
        writeln!(
            out,
            "- See the approved work order for the remaining acceptance criteria."
        )?;
    }
    writeln!(out, "\n### Candidate diff\n")?;
    print_diff(out, &changes, 8)
}

pub fn write_repair_comparison(
    out: &mut impl std::io::Write,
    order: &crate::order::WorkOrder,
    prior: &str,
    head: &str,
) -> Result<()> {
    let repair = crate::repository::revision_diff(order, prior, head)?;
    writeln!(out, "Changes since reviewed revision `{prior}`\n")?;
    print_diff(out, &repair, 5)
}

pub fn print_brief(receipt: &Receipt) -> Result<()> {
    use std::io::Write;

    let head = receipt
        .candidate
        .as_deref()
        .ok_or("submit a candidate before generating a brief")?;
    crate::repository::candidate_paths(&receipt.work_order, head)?;
    let mut out = std::io::stdout().lock();
    write_candidate_summary(&mut out, &receipt.work_order, head)?;
    if let Some(request) = receipt
        .attestations
        .iter()
        .rev()
        .find(|entry| entry.kind == "changes_requested_attested")
    {
        let prior = &request.record.draft.head;
        writeln!(
            out,
            "\n### Review feedback\n\n{}\n",
            markdown(&request.record.note, 350)
        )?;
        if prior != head
            && write_repair_comparison(&mut out, &receipt.work_order, prior, head).is_err()
        {
            writeln!(out, "Repair comparison unavailable in this checkout.")?;
        }
    }
    writeln!(out, "\n### Verification\n")?;
    if receipt.publication_hold.is_some() {
        writeln!(
            out,
            "- Publication outcome is unresolved. Reconcile the forge state before requesting review."
        )?;
        return Ok(());
    }
    if let Some(problem) = &receipt.snapshot_problem {
        let provider = if crate::snapshot::reference(&receipt.work_order)
            .ok()
            .flatten()
            .is_some_and(crate::snapshot::is_github_issue)
        {
            "GitHub issue"
        } else {
            "Jira"
        };
        writeln!(
            out,
            "- Review readiness is blocked. Restore or review the pinned {provider} snapshot. {}",
            markdown(problem, 300)
        )?;
    }
    let live = receipt.events.iter().rev().find(|event| {
        event.candidate.as_deref() == Some(head)
            && matches!(
                event.kind.as_str(),
                "github_checks_observed"
                    | "github_checks_unavailable"
                    | "bitbucket_checks_observed"
                    | "bitbucket_checks_unavailable"
                    | "native_check_started"
            )
    });
    if let Some(event) = live.filter(|event| event.kind == "github_checks_observed") {
        let observation: serde_json::Value = serde_json::from_str(&event.note)?;
        writeln!(
            out,
            "- Required GitHub checks succeeded at the last observation. Confirm current results in the forge."
        )?;
        if let Some(checks) = observation["checks"].as_array() {
            for check in checks {
                let name = check["name"]
                    .as_str()
                    .ok_or("stored GitHub check is missing its name")?;
                let id = check["id"]
                    .as_u64()
                    .ok_or("stored GitHub check is missing its ID")?;
                writeln!(out, "  - {} (run {id})", markdown(name, 200))?;
            }
        }
    } else if let Some(event) = live.filter(|event| event.kind == "bitbucket_checks_observed") {
        let observation: serde_json::Value = serde_json::from_str(&event.note)?;
        let pipeline = observation["pipeline"]
            .as_str()
            .ok_or("stored observation is missing the Bitbucket pipeline")?;
        writeln!(
            out,
            "- Required Bitbucket pipeline steps succeeded for this revision. Pipeline {}.",
            markdown(pipeline, 100)
        )?;
        if observation["transport"] == "twg" {
            writeln!(
                out,
                "- TWG confirmed the PR and both branch heads. Refresh verification before using this evidence for a decision."
            )?;
        } else {
            writeln!(
                out,
                "- Craft reported successful pipeline steps. Run check-bitbucket-twg to verify the live PR target and pinned base before review."
            )?;
        }
        if let Some(checks) = observation["checks"].as_array() {
            for check in checks {
                let name = check["name"]
                    .as_str()
                    .ok_or("stored Bitbucket step is missing its name")?;
                writeln!(out, "  - {}", markdown(name, 200))?;
            }
        }
    } else {
        writeln!(out, "- Current provider checks are pending or unavailable.")?;
    }
    if matches!(
        receipt.native_ci,
        crate::store::NativeCi::Required
            | crate::store::NativeCi::Pending
            | crate::store::NativeCi::Unavailable
    ) {
        writeln!(
            out,
            "- Review readiness is blocked pending a successful native check for this revision."
        )?;
    }
    if let Some(verification) = receipt
        .attestations
        .iter()
        .rev()
        .find(|entry| entry.kind == "verification_attested" && entry.record.draft.head == head)
    {
        let description = match verification.record.source {
            EvidenceSource::Unspecified => "Verification recorded with unspecified provenance",
            EvidenceSource::OperatorObserved => "Verification attested by the operator",
            EvidenceSource::Simulated => "Simulated verification recorded",
        };
        writeln!(
            out,
            "- {description}. Actor {}. {}",
            markdown(&verification.record.actor, 100),
            markdown(&verification.record.note, 500)
        )?;
        writeln!(
            out,
            "- Evidence {}",
            markdown(&verification.record.evidence, 200)
        )?;
    } else {
        writeln!(out, "- Independent verification is pending.")?;
    }
    if receipt.attestations.iter().any(|entry| {
        entry.kind == "changes_requested_attested"
            && entry.record.source == EvidenceSource::Simulated
    }) {
        writeln!(out, "- This workflow includes a simulated change request.")?;
    }
    if let Some(merge) = &receipt.native_merge {
        writeln!(
            out,
            "- Merged into **{}**. [Result commit]({}).",
            markdown(&merge.destination_branch, 100),
            merge.reference
        )?;
    }
    Ok(())
}

fn print_diff(
    out: &mut impl std::io::Write,
    files: &[crate::repository::DiffFile],
    limit: usize,
) -> Result<()> {
    let (added, removed, binary) =
        files
            .iter()
            .fold((0, 0, 0), |(added, removed, binary), file| {
                match file.lines {
                    Some((a, r)) => (added + a, removed + r, binary),
                    None => (added, removed, binary + 1),
                }
            });
    let suffix = if files.len() == 1 { "" } else { "s" };
    write!(
        out,
        "{} file{suffix} changed · +{added} / -{removed} lines",
        files.len()
    )?;
    if binary > 0 {
        let suffix = if binary == 1 { "" } else { "s" };
        write!(out, " · {binary} binary file{suffix}")?;
    }
    writeln!(out, "\n")?;
    let mut ranked: Vec<_> = files.iter().collect();
    ranked.sort_by_key(|file| {
        std::cmp::Reverse(
            file.lines
                .map_or(u64::MAX, |(added, removed)| added + removed),
        )
    });
    for file in ranked.into_iter().take(limit) {
        match file.lines {
            Some((added, removed)) => writeln!(
                out,
                "- {} (+{added} / -{removed})",
                markdown(&file.path, 180)
            )?,
            None => writeln!(out, "- {} (binary)", markdown(&file.path, 180))?,
        }
    }
    if files.len() > limit {
        writeln!(out, "- {} more in the full diff.", files.len() - limit)?;
    }
    Ok(())
}

fn markdown(value: &str, limit: usize) -> String {
    let condensed: String = value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|character| !character.is_control())
        .collect();
    let mut result = String::new();
    for character in condensed.chars().take(limit) {
        match character {
            '&' => result.push_str("&amp;"),
            '<' => result.push_str("&lt;"),
            '>' => result.push_str("&gt;"),
            '@' => result.push_str("&#64;"),
            '\\' | '`' | '*' | '_' | '[' | ']' | '~' | '|' => {
                result.push('\\');
                result.push(character);
            }
            _ => result.push(character),
        }
    }
    if condensed.chars().count() > limit {
        result.push('…');
    }
    result
}
