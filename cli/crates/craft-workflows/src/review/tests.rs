use super::*;
use crate::plans::{DocumentTarget, JiraMapping, PlanTarget, PlanTask, RichText};
use craft_core::control::CancellationToken;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
type TestResult = Result<(), Box<dyn std::error::Error>>;
const CLOUD: &str = "00000000-1111-2222-3333-444444444444";
fn target(provider: &str, kind: &str, id: &str) -> ResourceRef {
    ResourceRef {
        provider: provider.into(),
        deployment: "cloud".into(),
        tenant: CLOUD.into(),
        resource_type: kind.into(),
        id: id.into(),
    }
}
fn hash(letter: char) -> String {
    format!("sha256:{}", letter.to_string().repeat(64))
}
fn plan() -> Result<Plan, CraftError> {
    let mut p = Plan::scaffold(
        "invoice-plan".into(),
        "Invoice review".into(),
        PlanTarget {
            confluence: DocumentTarget {
                confluence_cloud_id: CLOUD.into(),
                space_id: "100".into(),
                parent_page_id: None,
            },
            jira_cloud_id: CLOUD.into(),
            project_key: "PAY".into(),
        },
    )?;
    p.context = RichText::Markdown("Humans review the complete plan.".into());
    p.designs.push(DesignReference {
        design_id: "ledger-design".into(),
        cloud_id: CLOUD.into(),
        page_id: "102".into(),
        version: 3,
        digest: hash('d'),
    });
    p.tasks.push(PlanTask {
        task_id: "contract".into(),
        title: "Define identity".into(),
        jira: JiraMapping {
            project_key: "PAY".into(),
            issue_type: "Task".into(),
            existing_issue: None,
        },
        description: RichText::Markdown("Keep scope explicit.".into()),
        acceptance: vec![RichText::Markdown(
            "One operation retains one identity.".into(),
        )],
        repositories: vec!["api".into()],
        depends_on: vec![],
        designs: vec!["ledger-design".into()],
        include_in_jira: true,
        estimate: None,
    });
    Ok(p)
}
fn binding() -> Result<ReviewBinding, CraftError> {
    ReviewBinding::from_plan(
        &plan()?,
        target("confluence", "page", "101"),
        7,
        Representation::Storage,
        hash('a'),
        BTreeMap::from([("PAY".into(), target("jira", "project", "200"))]),
        hash('b'),
    )
}
fn policy(quorum: usize) -> Result<TrustedReviewerPolicy, CraftError> {
    TrustedReviewerPolicy::from_host_configuration(
        1,
        target("confluence", "space", "100"),
        "worker-agent".into(),
        vec!["alice".into(), "bob".into()],
        vec!["other-agent".into()],
        quorum,
        true,
    )
}
fn comment(
    id: &str,
    actor: &str,
    decision: Decision,
    b: &ReviewBinding,
    adf: bool,
) -> Result<CommentObservation, Box<dyn std::error::Error>> {
    let text = serde_json::to_string(&ReviewStatement::new(decision, b.clone())?)?;
    let document = Document::from_markdown(&format!("```json\n{text}\n```"))?;
    Ok(CommentObservation {
        id: id.into(),
        page: b.page.clone(),
        status: "current".into(),
        version: 1,
        version_author_id: actor.into(),
        version_created_at: "2026-11-10T00:00:00.000Z".into(),
        body: if adf {
            CommentBody::Adf(document.to_adf()?)
        } else {
            CommentBody::Storage(document.to_storage()?)
        },
    })
}
struct TestClock {
    wall: AtomicU64,
    mono: AtomicU64,
}
impl TestClock {
    fn new() -> Self {
        Self {
            wall: AtomicU64::new(1000),
            mono: AtomicU64::new(1000),
        }
    }
}
impl Clock for TestClock {
    fn unix_millis(&self) -> u64 {
        self.wall.load(Ordering::SeqCst)
    }
    fn monotonic_millis(&self) -> u64 {
        self.mono.load(Ordering::SeqCst)
    }
}
struct Port {
    actor: String,
    actor_after: Option<String>,
    actor_reads: usize,
    policy: TrustedReviewerPolicy,
    binding: ReviewBinding,
    comments: Vec<CommentObservation>,
    coverage: Coverage,
    policy_after: Option<TrustedReviewerPolicy>,
    binding_after: Option<ReviewBinding>,
    binding_final: Option<ReviewBinding>,
    comments_after: Option<Vec<CommentObservation>>,
    policy_reads: usize,
    binding_reads: usize,
    comment_reads: usize,
    clock: Arc<TestClock>,
    clock_change: Option<(u64, u64)>,
    cancel: Option<CancellationToken>,
    fail_comments: bool,
}
impl Port {
    fn new(quorum: usize) -> Result<Self, Box<dyn std::error::Error>> {
        let b = binding()?;
        Ok(Self {
            actor: "worker-agent".into(),
            actor_after: None,
            actor_reads: 0,
            policy: policy(quorum)?,
            binding: b.clone(),
            comments: vec![
                comment("301", "alice", Decision::Approve, &b, false)?,
                comment("302", "bob", Decision::Approve, &b, true)?,
            ],
            coverage: Coverage::Complete,
            policy_after: None,
            binding_after: None,
            binding_final: None,
            comments_after: None,
            policy_reads: 0,
            binding_reads: 0,
            comment_reads: 0,
            clock: Arc::new(TestClock::new()),
            clock_change: None,
            cancel: None,
            fail_comments: false,
        })
    }
    fn control(&self) -> Result<InvocationControl, CraftError> {
        InvocationControl::new(self.clock.clone(), CancellationToken::default(), 60_000)
    }
}
impl ReviewEvidencePort for Port {
    fn execution_actor(&mut self) -> Result<String, CraftError> {
        self.actor_reads += 1;
        Ok(if self.actor_reads > 1 {
            self.actor_after
                .clone()
                .unwrap_or_else(|| self.actor.clone())
        } else {
            self.actor.clone()
        })
    }
    fn policy(&mut self) -> Result<TrustedReviewerPolicy, CraftError> {
        self.policy_reads += 1;
        Ok(if self.policy_reads > 1 {
            self.policy_after
                .clone()
                .unwrap_or_else(|| self.policy.clone())
        } else {
            self.policy.clone()
        })
    }
    fn binding(&mut self) -> Result<ReviewBinding, CraftError> {
        self.binding_reads += 1;
        if self.binding_reads == 3 {
            if let Some((wall, mono)) = self.clock_change {
                self.clock.wall.store(wall, Ordering::SeqCst);
                self.clock.mono.store(mono, Ordering::SeqCst);
            }
            if let Some(c) = &self.cancel {
                c.cancel();
            }
        }
        Ok(if self.binding_reads >= 3 && self.binding_final.is_some() {
            self.binding_final.clone().ok_or_else(changed)?
        } else if self.binding_reads > 1 {
            self.binding_after
                .clone()
                .unwrap_or_else(|| self.binding.clone())
        } else {
            self.binding.clone()
        })
    }
    fn comments(&mut self) -> Result<CommentInventory, CraftError> {
        self.comment_reads += 1;
        if self.fail_comments {
            return Err(CraftError::new(
                ErrorCode::PermissionDenied,
                "Review comments are unavailable.",
            ));
        }
        Ok(CommentInventory {
            coverage: self.coverage,
            comments: if self.comment_reads > 1 {
                self.comments_after
                    .clone()
                    .unwrap_or_else(|| self.comments.clone())
            } else {
                self.comments.clone()
            },
        })
    }
}
fn checked(port: &mut Port) -> Result<VerifiedReview, CraftError> {
    let clock = port.clock.clone();
    let control = port.control()?;
    verify(port, clock.as_ref(), &control)
}

#[test]
fn exact_binding_and_distinct_human_versions_are_observed_twice() -> TestResult {
    let mut port = Port::new(2)?;
    port.comments.push(comment(
        "303",
        "worker-agent",
        Decision::Approve,
        &port.binding,
        false,
    )?);
    let result = checked(&mut port)?;
    assert_eq!(result.approvals().len(), 2);
    assert!(result.approvals().iter().all(|c| c.actor != "worker-agent"));
    assert_eq!(
        (port.policy_reads, port.binding_reads, port.comment_reads),
        (2, 3, 2)
    );
    let receipt = serde_json::to_value(&result)?;
    assert_eq!(receipt["binding"]["version"], 7);
    assert_eq!(receipt["binding"]["designs"][0]["version"], 3);
    assert_eq!(receipt["binding"]["jira_targets"][0]["id"], "200");
    let mut reversed = Port::new(2)?;
    let mut after = reversed.comments.clone();
    after.reverse();
    reversed.comments_after = Some(after);
    checked(&mut reversed)?;
    Ok(())
}
#[test]
fn unknown_partial_worker_and_unresolved_evidence_cannot_authorize() -> TestResult {
    for mode in [
        "partial",
        "unknown",
        "worker",
        "unlisted",
        "duplicate_actor",
        "duplicate_comment",
        "request",
        "stale_request",
        "withdraw",
        "unstructured",
        "unknown_field",
        "wrong_page",
        "historical",
        "missing_actor",
        "missing_time",
        "zero_version",
        "oversized",
        "read_error",
        "stale_approval",
    ] {
        let mut port = Port::new(2)?;
        match mode {
            "partial" => port.coverage = Coverage::Partial,
            "unknown" => port.coverage = Coverage::Unknown,
            "worker" => {
                for c in &mut port.comments {
                    c.version_author_id = "worker-agent".into();
                }
            }
            "unlisted" => port.comments[0].version_author_id = "unlisted-person".into(),
            "duplicate_actor" => port.comments[1].version_author_id = "alice".into(),
            "duplicate_comment" => port.comments.push(port.comments[0].clone()),
            "request" | "stale_request" => {
                let mut b = port.binding.clone();
                if mode == "stale_request" {
                    b.version = 6;
                }
                port.comments.push(comment(
                    "304",
                    "alice",
                    Decision::ChangesRequested,
                    &b,
                    false,
                )?);
            }
            "withdraw" => port.comments.push(comment(
                "304",
                "alice",
                Decision::Withdraw,
                &port.binding,
                false,
            )?),
            "unstructured" => {
                port.comments[0].body =
                    CommentBody::Storage("<p>Changes requested, please fix the design.</p>".into())
            }
            "unknown_field" => {
                let mut v = json!(ReviewStatement::new(
                    Decision::Approve,
                    port.binding.clone()
                )?);
                v["author_id"] = json!("alice");
                port.comments[0].body = CommentBody::Adf(
                    Document::from_markdown(&serde_json::to_string(&v)?)?.to_adf()?,
                );
            }
            "wrong_page" => port.comments[0].page.id = "999".into(),
            "historical" => port.comments[0].status = "historical".into(),
            "missing_actor" => port.comments[0].version_author_id = "unknown".into(),
            "missing_time" => port.comments[0].version_created_at.clear(),
            "zero_version" => port.comments[0].version = 0,
            "oversized" => {
                port.comments[0].body = CommentBody::Storage("x".repeat(MAX_BODY_BYTES + 1))
            }
            "read_error" => port.fail_comments = true,
            _ => {
                let mut stale = port.binding.clone();
                stale.mapping_digest = hash('f');
                port.comments[0] = comment("301", "alice", Decision::Approve, &stale, false)?;
            }
        }
        assert!(checked(&mut port).is_err(), "{mode}");
    }
    Ok(())
}
#[test]
fn page_design_mapping_policy_and_comment_drift_stop_verification() -> TestResult {
    for mode in [
        "page_version",
        "page_digest",
        "plan_digest",
        "mapping",
        "design",
        "jira",
        "policy",
        "comment_body",
        "comment_author",
        "comment_version",
        "deleted_comment",
    ] {
        let mut port = Port::new(2)?;
        let mut b = port.binding.clone();
        let mut comments = port.comments.clone();
        match mode {
            "page_version" => b.version += 1,
            "page_digest" => b.page_digest = hash('e'),
            "plan_digest" => b.document_digest = hash('e'),
            "mapping" => b.mapping_digest = hash('e'),
            "design" => b.designs[0].version += 1,
            "jira" => b.jira_targets[0].id = "999".into(),
            "policy" => {
                let mut p = port.policy.clone();
                p.revision = 2;
                port.policy_after = Some(p);
            }
            "comment_body" => comments[0].body = CommentBody::Storage("<p>Withdrawn</p>".into()),
            "comment_author" => comments[0].version_author_id = "worker-agent".into(),
            "comment_version" => comments[0].version += 1,
            _ => {
                comments.remove(0);
            }
        }
        if matches!(
            mode,
            "page_version" | "page_digest" | "plan_digest" | "mapping" | "design" | "jira"
        ) {
            port.binding_after = Some(b);
        } else if mode != "policy" {
            port.comments_after = Some(comments);
        }
        assert!(checked(&mut port).is_err(), "{mode}");
    }
    Ok(())
}
#[test]
fn freshness_backward_clocks_and_cancellation_fail_closed() -> TestResult {
    for times in [
        (31_000, 31_000),
        (31_000, 1000),
        (1000, 31_000),
        (999, 1000),
        (1000, 999),
    ] {
        let mut port = Port::new(2)?;
        port.clock_change = Some(times);
        assert!(checked(&mut port).is_err(), "{times:?}");
    }
    let mut port = Port::new(2)?;
    let cancellation = CancellationToken::default();
    let control = InvocationControl::new(port.clock.clone(), cancellation.clone(), 60_000)?;
    port.cancel = Some(cancellation);
    let clock = port.clock.clone();
    assert_eq!(
        verify(&mut port, clock.as_ref(), &control)
            .err()
            .ok_or("cancellation")?
            .code,
        ErrorCode::Cancelled
    );
    Ok(())
}
#[test]
fn revalidation_never_replaces_a_deleted_or_edited_original_approval() -> TestResult {
    let mut initial = Port::new(1)?;
    initial.comments.truncate(1);
    let approved = checked(&mut initial)?;
    for mode in ["extra", "extra_same_actor", "replacement", "edited"] {
        let mut now = Port::new(1)?;
        now.comments.truncate(1);
        if mode == "edited" {
            now.comments[0].version = 2;
        }
        if mode == "replacement" {
            now.comments.clear();
        }
        now.comments.push(comment(
            "305",
            if mode == "extra_same_actor" {
                "alice"
            } else {
                "bob"
            },
            Decision::Approve,
            &now.binding,
            false,
        )?);
        let clock = now.clock.clone();
        let control = now.control()?;
        let result = revalidate(&approved, &mut now, clock.as_ref(), &control);
        assert_eq!(
            result.is_ok(),
            matches!(mode, "extra" | "extra_same_actor"),
            "{mode}"
        );
    }
    Ok(())
}
#[test]
fn policy_and_binding_construction_reject_self_approval_and_incomplete_mappings() -> TestResult {
    for (reviewers, excluded, quorum) in [
        (vec!["worker-agent"], vec![], 1),
        (vec!["alice", "alice"], vec![], 1),
        (vec!["alice"], vec!["alice"], 1),
        (vec!["alice"], vec![], 2),
        (vec!["unknown"], vec![], 1),
    ] {
        assert!(
            TrustedReviewerPolicy::from_host_configuration(
                1,
                target("confluence", "space", "100"),
                "worker-agent".into(),
                reviewers.into_iter().map(String::from).collect(),
                excluded.into_iter().map(String::from).collect(),
                quorum,
                true
            )
            .is_err()
        );
    }
    let p = plan()?;
    assert!(
        ReviewBinding::from_plan(
            &p,
            target("confluence", "page", "101"),
            7,
            Representation::Storage,
            hash('a'),
            BTreeMap::new(),
            hash('b')
        )
        .is_err()
    );
    let mut project = target("jira", "project", "200");
    project.tenant = "00000000-1111-2222-3333-555555555555".into();
    assert!(
        ReviewBinding::from_plan(
            &p,
            target("confluence", "page", "101"),
            7,
            Representation::Storage,
            hash('a'),
            BTreeMap::from([("PAY".into(), project)]),
            hash('b')
        )
        .is_err()
    );
    let mut port = Port::new(2)?;
    port.policy.enabled = false;
    assert!(checked(&mut port).is_err());
    let mut port = Port::new(2)?;
    port.actor = "alice".into();
    assert!(checked(&mut port).is_err());
    let mut port = Port::new(2)?;
    port.actor_after = Some("alice".into());
    assert!(checked(&mut port).is_err());
    Ok(())
}

#[test]
fn duplicate_members_marks_and_extra_visible_blocks_never_count_as_approval() -> TestResult {
    let text = serde_json::to_string(&ReviewStatement::new(Decision::Approve, binding()?)?)?;
    for key in ["decision", "format"] {
        let mut duplicate = text.clone();
        duplicate.pop();
        duplicate.push_str(if key == "decision" {
            ",\"decision\":\"approve\"}"
        } else {
            ",\"format\":\"craft.review/v1\"}"
        });
        let body = CommentBody::Storage(
            Document::from_markdown(&format!("```json\n{duplicate}\n```"))?.to_storage()?,
        );
        assert!(statement(&body).is_err(), "{key}");
    }
    let marked = CommentBody::Adf(
        json!({"version":1,"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":text,"marks":[{"type":"strong"}]}]}]}),
    );
    assert!(body_value(&marked).is_ok());
    assert!(statement(&marked).is_err());
    let multiple = CommentBody::Adf(
        json!({"version":1,"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":text}]},{"type":"paragraph","content":[{"type":"text","text":"Pending changes"}]}]}),
    );
    assert!(body_value(&multiple).is_ok());
    assert!(statement(&multiple).is_err());
    Ok(())
}

fn sized_body(bytes: usize) -> Result<CommentBody, Box<dyn std::error::Error>> {
    let overhead = serde_json::to_vec(&CommentBody::Storage(String::new()))?.len();
    if bytes < overhead {
        return Err("body size below encoding overhead".into());
    }
    let body = CommentBody::Storage("x".repeat(bytes - overhead));
    assert_eq!(serde_json::to_vec(&body)?.len(), bytes);
    Ok(body)
}
#[test]
fn exact_comment_count_body_and_total_inventory_limits_are_enforced() -> TestResult {
    let b = binding()?;
    let prototype = comment("1000", "worker-agent", Decision::Approve, &b, false)?;
    let mut count = CommentInventory {
        coverage: Coverage::Complete,
        comments: (0..MAX_COMMENTS)
            .map(|i| {
                let mut c = prototype.clone();
                c.id = (1000 + i).to_string();
                c
            })
            .collect(),
    };
    inventory_digest(&count, &b)?;
    let mut extra = prototype.clone();
    extra.id = "99999".into();
    count.comments.push(extra);
    assert!(inventory_digest(&count, &b).is_err());
    let mut one = prototype.clone();
    one.body = sized_body(MAX_BODY_BYTES)?;
    let mut body_limit = CommentInventory {
        coverage: Coverage::Complete,
        comments: vec![one],
    };
    inventory_digest(&body_limit, &b)?;
    body_limit.comments[0].body = sized_body(MAX_BODY_BYTES + 1)?;
    assert!(inventory_digest(&body_limit, &b).is_err());
    let mut bytes = MAX_INVENTORY_BYTES;
    let mut rows = Vec::new();
    let mut id = 1000;
    while bytes > 0 {
        let size = bytes.min(MAX_BODY_BYTES);
        let mut c = prototype.clone();
        c.id = id.to_string();
        c.body = sized_body(size)?;
        rows.push(c);
        bytes -= size;
        id += 1;
    }
    let mut total = CommentInventory {
        coverage: Coverage::Complete,
        comments: rows,
    };
    inventory_digest(&total, &b)?;
    let mut c = prototype;
    c.id = "99999".into();
    c.body = sized_body(32)?;
    total.comments.push(c);
    assert!(inventory_digest(&total, &b).is_err());
    Ok(())
}
#[test]
fn drift_only_at_the_final_binding_read_still_invalidates_review() -> TestResult {
    let mut port = Port::new(2)?;
    let mut changed_binding = port.binding.clone();
    changed_binding.page_digest = hash('e');
    port.binding_final = Some(changed_binding);
    assert_eq!(
        checked(&mut port).err().ok_or("final drift")?.code,
        ErrorCode::VersionConflict
    );
    assert_eq!(
        (port.policy_reads, port.binding_reads, port.comment_reads),
        (2, 3, 2)
    );
    Ok(())
}

#[test]
fn disabled_review_policy_refuses_before_identity_and_resource_reads() -> TestResult {
    let mut port = Port::new(2)?;
    port.policy.enabled = false;
    assert!(checked(&mut port).is_err());
    assert_eq!(
        (port.actor_reads, port.binding_reads, port.comment_reads),
        (0, 0, 0)
    );
    Ok(())
}
