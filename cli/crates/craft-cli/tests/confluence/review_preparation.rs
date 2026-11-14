use super::*;
use craft_cli::planning_review::command::{PrepareArgs, prepare_native};
use craft_cli::planning_review::{BindingEnvironment, LiveBindingHost};
use craft_core::storage::{
    ArtifactBlob, ArtifactReceipt, ArtifactSpec, ArtifactStore, ArtifactWriter, StoreError,
};
use craft_runtime::artifacts::NativeArtifacts;
use craft_runtime::create_policy::CreatePolicyUpdate;
use craft_runtime::review_policy::ReviewPolicyUpdate;
use craft_workflows::plans::*;
use std::io::{self, Write};

fn jira_target() -> ResourceRef {
    ResourceRef {
        provider: "jira".into(),
        resource_type: "project".into(),
        id: "10000".into(),
        ..target()
    }
}
pub(super) fn configure(setup: &Setup, enabled: bool) -> Result<GrantRef, CraftError> {
    enable_reads(setup)?;
    setup.store.set_review_policy(
        "factory",
        ReviewPolicyUpdate {
            target: ResourceRef {
                resource_type: "space".into(),
                id: "100".into(),
                ..target()
            },
            expected_actor: "actor-one".into(),
            reviewers: vec!["human-one".into()],
            excluded_actors: vec![],
            quorum: 1,
            enabled,
            expected_revision: None,
        },
        true,
        &control()?,
        |_| Ok(()),
    )?;
    let profile = setup.store.create(
        &ProfileInput {
            name: "jira-review".into(),
            provider: "jira".into(),
            deployment: "cloud".into(),
            auth_mode: AuthMode::Service,
            tenant: Some(CLOUD.into()),
            grant: None,
            defaults: BTreeMap::new(),
        },
        &control()?,
        |_| Ok(()),
    )?;
    setup.store.set_create_policy(
        "jira-review",
        CreatePolicyUpdate {
            target: jira_target(),
            expected_actor: "actor-one".into(),
            enabled: true,
            expected_revision: None,
        },
        true,
        &control()?,
        |_| Ok(()),
    )?;
    Ok(profile.grant)
}
pub(super) fn sources(
    stale: bool,
    unsupported: bool,
) -> Result<(Value, Value), Box<dyn std::error::Error>> {
    let destination = DocumentTarget {
        confluence_cloud_id: CLOUD.into(),
        space_id: "100".into(),
        parent_page_id: None,
    };
    let mut design = Design::scaffold(
        "ledger-contract".into(),
        "Ledger contract".into(),
        DesignKind::Hld,
        destination.clone(),
        true,
    )?;
    for section in &mut design.sections {
        section.body = RichText::Markdown(format!("The reviewed {} contract.", section.title));
    }
    let mut plan = Plan::scaffold(
        "ledger-plan".into(),
        "Ledger plan".into(),
        PlanTarget {
            confluence: destination,
            jira_cloud_id: CLOUD.into(),
            project_key: "PAY".into(),
        },
    )?;
    plan.context = RichText::Markdown("Human reviewed context.".into());
    plan.designs = vec![DesignReference {
        design_id: design.design_id.clone(),
        cloud_id: CLOUD.into(),
        page_id: "301".into(),
        version: if stale { 1 } else { 2 },
        digest: design.digest()?,
    }];
    for (id, title, depends) in [
        ("define-contract", "Define contract", vec![]),
        (
            "implement-contract",
            "Implement contract",
            vec!["define-contract".into()],
        ),
    ] {
        plan.tasks.push(PlanTask {
            task_id: id.into(),
            title: title.into(),
            jira: JiraMapping {
                project_key: "PAY".into(),
                issue_type: "Task".into(),
                existing_issue: None,
            },
            description: RichText::Markdown("Keep the **contract** and rich source text.".into()),
            acceptance: vec![RichText::Markdown("No duplicate effect.".into())],
            repositories: vec!["api".into()],
            depends_on: depends,
            designs: vec![design.design_id.clone()],
            include_in_jira: true,
            estimate: None,
        });
    }
    let mut plan_body = plan.render()?.to_storage()?;
    if unsupported {
        plan_body.push_str("<ac:structured-macro ac:name=\"unsupported\" />");
    }
    let native = |id: u64, title: String, version: u64, body: String| json!({"id":id.to_string(),"spaceId":"100","status":"current","title":title,"version":{"number":version},"body":{"storage":{"representation":"storage","value":body}}});
    Ok((
        native(101, plan.title, 3, plan_body),
        native(301, design.title.clone(), 2, design.render()?.to_storage()?),
    ))
}
pub(super) fn args(wrong_set: bool) -> Result<PrepareArgs, serde_json::Error> {
    serde_json::from_value(
        json!({"review_id":"ledger-review","apply":true,"page_id":"101","space_id":"100","dependency_link_type_id":"10000","dependency_link_direction":"inward","tasks":if wrong_set {vec![json!({"task_id":"different","profile":"jira-review","project_id":"10000","input":{"issue_type_id":"10001","summary":"Different"}})]}else{vec![json!({"task_id":"define-contract","profile":"jira-review","project_id":"10000","input":{"issue_type_id":"10001","summary":"Define contract","description":"Proposed ticket description."}}),json!({"task_id":"implement-contract","profile":"jira-review","project_id":"10000","input":{"issue_type_id":"10001","summary":"Implement contract","description":"Implementation proposal."}})]}}),
    )
}
pub(super) fn jira_identity() -> Reply {
    Reply::json(200, br#"{"accountId":"actor-one","active":true}"#, &[])
}
pub(super) fn metadata(mode: &str, reverse: bool) -> Result<Vec<Reply>, serde_json::Error> {
    let field = |id: &str, kind: &str, required: bool| json!({"fieldId":id,"key":id,"name":id,"required":required,"operations":["set"],"schema":{"type":kind,"system":id}});
    let mut fields = vec![
        field("project", "project", true),
        field("issuetype", "issuetype", true),
        field("summary", "string", true),
        field("description", "string", false),
    ];
    if mode == "unreported_default" {
        let mut value = field("priority", "priority", false);
        value["hasDefaultValue"] = json!(true);
        fields.push(value);
    }
    if matches!(mode, "reported_default" | "changed_default") {
        let mut value = field("labels", "array", false);
        value["schema"]["items"] = json!("string");
        value["hasDefaultValue"] = json!(true);
        value["defaultValue"] = json!([if mode == "reported_default" {
            "first"
        } else {
            "changed"
        }]);
        fields.push(value);
    }
    if reverse {
        fields.reverse();
    }
    Ok(vec![
        reply(json!({"id":"10000","key":if mode=="wrong_key"{"OTHER"}else{"PAY"}}))?,
        reply(
            json!({"startAt":0,"maxResults":50,"total":1,"issueTypes":[{"id":"10001","name":if mode=="wrong_type"{"Bug"}else{"Task"},"subtask":false}]}),
        )?,
        reply(json!({"startAt":0,"maxResults":50,"total":fields.len(),"fields":fields}))?,
        reply(
            json!({"issueLinkTypes":[{"id":"10000","name":"Blocks","inward":"is blocked by","outward":if mode == "changed_link" {"now relates to"} else {"blocks"}}]}),
        )?,
    ])
}
pub(super) fn reads(
    mode: &str,
    plan: &Value,
    design: &Value,
    reverse: bool,
) -> Result<Vec<Reply>, serde_json::Error> {
    let mut replies = vec![identity(), reply(plan.clone())?];
    if matches!(mode, "wrong_taskset" | "unsupported_source") {
        return Ok(replies);
    }
    replies.extend([identity(), reply(design.clone())?]);
    if mode == "stale_design" {
        return Ok(replies);
    }
    replies.push(jira_identity());
    replies.extend(metadata(mode, reverse)?);
    if plan
        .pointer("/body/storage/value")
        .and_then(Value::as_str)
        .is_some_and(|body| body.contains("Reuse existing issue ID"))
    {
        replies.push(reply(json!({"id":if mode=="reuse_wrong_id" {"11999"} else {"11001"},"key":"PAY-11001","fields":{"project":{"id":if mode=="reuse_wrong_project" {"99999"} else {"10000"},"key":"PAY","name":"Payments"},"issuetype":{"id":if mode=="reuse_wrong_type" {"99999"} else {"10001"},"name":"Task","subtask":false}}}))?);
    }
    Ok(replies)
}
pub(super) fn add_reuse(plan: &mut Value) -> TestResult {
    let body = plan
        .pointer("/body/storage/value")
        .and_then(Value::as_str)
        .ok_or("plan body")?;
    let mut document =
        Plan::from_document(&craft_workflows::documents::Document::from_storage(body)?)?;
    document.tasks[0].jira.existing_issue = Some(craft_workflows::plans::ExistingIssue {
        issue_id: "11001".into(),
        origin_operation_id: "prior-unknown-create".into(),
    });
    plan["body"]["storage"]["value"] = json!(document.render()?.to_storage()?);
    Ok(())
}
fn wrong_reuse_observation(mode: &str) -> TestResult {
    let setup = Setup::new()?;
    let jira_grant = configure(&setup, true)?;
    let (mut plan, design) = sources(false, false)?;
    add_reuse(&mut plan)?;
    let mut replies = vec![identity(), jira_identity()];
    replies.extend(reads(mode, &plan, &design, false)?);
    let count = replies.len();
    let product = TlsServer::start(Reply::Sequence(replies))?;
    let issuer = issuer("read:confluence-user read:page:confluence read:jira-user read:jira-work")?;
    let clock = Arc::new(ManualClock::new(1000));
    let fixture = setup.fixture(&issuer, &product, clock.clone())?;
    for (grant, tenant) in [
        (&setup.grant, target().tenant_ref()),
        (&jira_grant, jira_target().tenant_ref()),
    ] {
        let locked = fixture.lock(grant, &control()?)?;
        fixture.client().provision_grant(
            &locked,
            fixture.service_credentials()?,
            &[tenant],
            Some("actor-one"),
        )?;
    }
    clock.advance(Duration::from_secs(3600))?;
    let artifacts =
        NativeArtifacts::open(&setup.root.join("reviews"), 32 * 1024 * 1024, &control()?)?;
    let (data, error) = prepare_native(
        args(false)?,
        "factory",
        "wrong-reuse-observation",
        BindingEnvironment {
            store: &setup.store,
            credentials: &fixture,
            client: fixture.client(),
            tls: TlsPolicy::AdditionalCaBundle(product.root()),
            control: control()?,
        },
        &artifacts,
    );
    assert!(error.is_some(), "{mode}");
    assert!(!data.prepared && !data.review_verified && data.tickets_created == 0);
    assert!(data.artifact.is_none() && data.statement.is_none());
    assert_eq!(data.credential_cache_updates.len(), 2);
    let requests = product.finish()?;
    assert_eq!(requests.len(), count);
    assert!(requests.iter().all(|r| r.method == "GET"));
    issuer.finish()?;
    setup.no_secrets()?;
    Ok(())
}
#[test]
fn reused_issue_wrong_identity_cannot_enter_human_review_binding() -> TestResult {
    wrong_reuse_observation("reuse_wrong_id")
}
#[test]
fn reused_issue_wrong_project_cannot_enter_human_review_binding() -> TestResult {
    wrong_reuse_observation("reuse_wrong_project")
}
#[test]
fn reused_issue_wrong_type_cannot_enter_human_review_binding() -> TestResult {
    wrong_reuse_observation("reuse_wrong_type")
}
struct Uncertain<'a>(&'a dyn ArtifactStore);
struct UncertainWriter(Box<dyn ArtifactWriter>);
impl Write for UncertainWriter {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        self.0.write(b)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}
impl ArtifactWriter for UncertainWriter {
    fn finish(self: Box<Self>) -> Result<ArtifactReceipt, StoreError> {
        self.0.finish()?;
        Err(StoreError::OutcomeUnknown)
    }
}
impl ArtifactStore for Uncertain<'_> {
    fn begin(&self, spec: ArtifactSpec) -> Result<Box<dyn ArtifactWriter>, StoreError> {
        Ok(Box::new(UncertainWriter(self.0.begin(spec)?)))
    }
    fn receipt(&self, name: &str) -> Result<Option<ArtifactReceipt>, StoreError> {
        self.0.receipt(name)
    }
    fn read(&self, name: &str, max: u64) -> Result<Option<ArtifactBlob>, StoreError> {
        self.0.read(name, max)
    }
}
#[test]
fn live_review_preparation_qualifies_sources_and_material_fields_without_a_jira_write() -> TestResult
{
    for mode in [
        "prepared",
        "stale_design",
        "wrong_key",
        "wrong_type",
        "unreported_default",
        "wrong_taskset",
        "unsupported_source",
        "unknown_artifact",
        "duplicate_artifact",
        "disabled",
    ] {
        let setup = Setup::new()?;
        let jira_grant = configure(&setup, mode != "disabled")?;
        let (plan, design) = sources(mode == "stale_design", mode == "unsupported_source")?;
        let mut replies = vec![identity(), jira_identity()];
        if !matches!(mode, "duplicate_artifact" | "disabled") {
            replies.extend(reads(mode, &plan, &design, false)?);
        }
        let count = replies.len();
        let product = TlsServer::start(Reply::Sequence(replies))?;
        let issuer =
            issuer("read:confluence-user read:page:confluence read:jira-user read:jira-work")?;
        let clock = Arc::new(ManualClock::new(1000));
        let fixture = setup.fixture(&issuer, &product, clock.clone())?;
        for (grant, tenant) in [
            (&setup.grant, target().tenant_ref()),
            (&jira_grant, jira_target().tenant_ref()),
        ] {
            let locked = fixture.lock(grant, &control()?)?;
            fixture.client().provision_grant(
                &locked,
                fixture.service_credentials()?,
                &[tenant],
                Some("actor-one"),
            )?;
        }
        clock.advance(Duration::from_secs(3600))?;
        let artifacts =
            NativeArtifacts::open(&setup.root.join("reviews"), 32 * 1024 * 1024, &control()?)?;
        if mode == "duplicate_artifact" {
            let mut writer = artifacts.begin(ArtifactSpec {
                name: "review-ledger-review.json".into(),
                media_type: "application/json".into(),
                max_bytes: 32,
                source: None,
            })?;
            writer.write_all(b"{}")?;
            writer.finish()?;
        }
        let uncertain = Uncertain(&artifacts);
        let store: &dyn ArtifactStore = if mode == "unknown_artifact" {
            &uncertain
        } else {
            &artifacts
        };
        let (data, error) = prepare_native(
            args(mode == "wrong_taskset")?,
            "factory",
            "review-preparation",
            BindingEnvironment {
                store: &setup.store,
                credentials: &fixture,
                client: fixture.client(),
                tls: TlsPolicy::AdditionalCaBundle(product.root()),
                control: control()?,
            },
            store,
        );
        assert!(!data.review_verified);
        assert_eq!(data.tickets_created, 0);
        assert_eq!(data.prepared, mode == "prepared", "{mode}: {error:?}");
        assert_eq!(error.is_none(), mode == "prepared", "{mode}");
        let expected_cache = if matches!(mode, "duplicate_artifact" | "disabled") {
            0
        } else if matches!(
            mode,
            "stale_design" | "wrong_taskset" | "unsupported_source"
        ) {
            1
        } else {
            2
        };
        assert_eq!(
            data.credential_cache_updates.len(),
            expected_cache,
            "{mode}"
        );
        assert!(
            data.credential_cache_updates
                .iter()
                .all(|r| r.write_state == craft_core::contracts::RequestState::Confirmed)
        );
        if mode == "prepared" {
            assert!(data.statement.is_some());
            assert!(data.artifact.is_some());
            assert_eq!(
                data.artifact_write_state,
                craft_core::contracts::RequestState::Confirmed
            );
            let blob = artifacts
                .read(&data.artifact_name, 32 * 1024 * 1024)?
                .ok_or("artifact")?;
            let pack: Value = serde_json::from_slice(&blob.bytes)?;
            assert_eq!(
                pack["snapshot"]["plan"]["document"]["tasks"]
                    .as_array()
                    .ok_or("tasks")?
                    .len(),
                2
            );
            let tasks = pack["snapshot"]["tasks"].as_array().ok_or("resolved")?;
            assert_eq!(tasks.len(), 2);
            assert_eq!(tasks[0]["metadata_digest"], tasks[1]["metadata_digest"]);
            assert_eq!(tasks[0]["payload"]["fields"]["summary"], "Define contract");
            assert_eq!(tasks[0]["payload"]["fields"]["description"]["type"], "doc");
        } else if mode == "unknown_artifact" {
            assert_eq!(
                data.artifact_write_state,
                craft_core::contracts::RequestState::SentUnknown
            );
            assert!(data.statement.is_none());
            assert_eq!(data.artifact.is_some(), mode == "duplicate_artifact");
            assert!(data.artifact.is_none());
            assert!(artifacts.receipt(&data.artifact_name)?.is_some());
            assert_eq!(
                error.ok_or("unknown effect")?.details.write_state,
                craft_core::contracts::RequestState::SentUnknown
            );
        } else {
            assert_eq!(
                data.artifact_write_state,
                craft_core::contracts::RequestState::NotSent
            );
            assert!(data.statement.is_none());
        }
        let requests = product.finish()?;
        assert_eq!(requests.len(), count, "{mode}");
        assert!(requests.iter().all(|r| r.method == "GET"));
        assert_eq!(issuer.finish()?.len(), 2 + expected_cache);
        setup.no_secrets()?;
    }
    Ok(())
}
#[test]
fn material_binding_ignores_observation_time_and_field_row_order() -> TestResult {
    let setup = Setup::new()?;
    let jira_grant = configure(&setup, true)?;
    let (plan, design) = sources(false, false)?;
    let mut replies = vec![identity(), jira_identity()];
    replies.extend(reads("prepared", &plan, &design, false)?);
    replies.extend(reads("prepared", &plan, &design, true)?);
    let count = replies.len();
    let product = TlsServer::start(Reply::Sequence(replies))?;
    let issuer = issuer("read:confluence-user read:page:confluence read:jira-user read:jira-work")?;
    let clock = Arc::new(ManualClock::new(1000));
    let fixture = setup.fixture(&issuer, &product, clock.clone())?;
    for (grant, tenant) in [
        (&setup.grant, target().tenant_ref()),
        (&jira_grant, jira_target().tenant_ref()),
    ] {
        let locked = fixture.lock(grant, &control()?)?;
        fixture.client().provision_grant(
            &locked,
            fixture.service_credentials()?,
            &[tenant],
            Some("actor-one"),
        )?;
    }
    clock.advance(Duration::from_secs(3600))?;
    let mut host = LiveBindingHost::new(
        BindingEnvironment {
            store: &setup.store,
            credentials: &fixture,
            client: fixture.client(),
            tls: TlsPolicy::AdditionalCaBundle(product.root()),
            control: control()?,
        },
        "factory",
        "stable-binding",
        args(false)?.binding,
    )?;
    let first = host.compile()?;
    clock.advance(Duration::from_secs(1))?;
    let second = host.compile()?;
    assert_ne!(
        first.plan.source.observed_at,
        second.plan.source.observed_at
    );
    assert_eq!(
        craft_core::input::canonical_digest(&serde_json::to_value(first.binding)?)?,
        craft_core::input::canonical_digest(&serde_json::to_value(second.binding)?)?
    );
    assert_eq!(product.finish()?.len(), count);
    assert_eq!(issuer.finish()?.len(), 4);
    setup.no_secrets()?;
    Ok(())
}

#[test]
fn changed_reported_defaults_change_the_material_review_binding() -> TestResult {
    let setup = Setup::new()?;
    let jira_grant = configure(&setup, true)?;
    let (plan, design) = sources(false, false)?;
    let mut replies = vec![identity(), jira_identity()];
    replies.extend(reads("reported_default", &plan, &design, false)?);
    replies.extend(reads("changed_default", &plan, &design, true)?);
    let count = replies.len();
    let product = TlsServer::start(Reply::Sequence(replies))?;
    let issuer = issuer("read:confluence-user read:page:confluence read:jira-user read:jira-work")?;
    let clock = Arc::new(ManualClock::new(1000));
    let fixture = setup.fixture(&issuer, &product, clock.clone())?;
    for (grant, tenant) in [
        (&setup.grant, target().tenant_ref()),
        (&jira_grant, jira_target().tenant_ref()),
    ] {
        let locked = fixture.lock(grant, &control()?)?;
        fixture.client().provision_grant(
            &locked,
            fixture.service_credentials()?,
            &[tenant],
            Some("actor-one"),
        )?;
    }
    clock.advance(Duration::from_secs(3600))?;
    let mut host = LiveBindingHost::new(
        BindingEnvironment {
            store: &setup.store,
            credentials: &fixture,
            client: fixture.client(),
            tls: TlsPolicy::AdditionalCaBundle(product.root()),
            control: control()?,
        },
        "factory",
        "default-binding",
        args(false)?.binding,
    )?;
    let first = host.compile()?;
    let second = host.compile()?;
    assert_ne!(first.binding.mapping_digest, second.binding.mapping_digest);
    assert_eq!(
        first.tasks[0].metadata["field_details"]["labels"]["default_value"],
        json!(["first"])
    );
    assert_eq!(
        second.tasks[0].metadata["field_details"]["labels"]["default_value"],
        json!(["changed"])
    );
    assert_eq!(product.finish()?.len(), count);
    assert_eq!(issuer.finish()?.len(), 4);
    setup.no_secrets()?;
    Ok(())
}
struct RevokeAfterPublish<'a> {
    store: &'a dyn ArtifactStore,
    root: PathBuf,
}
struct RevokingWriter {
    writer: Box<dyn ArtifactWriter>,
    root: PathBuf,
}
impl Write for RevokingWriter {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        self.writer.write(b)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}
impl ArtifactWriter for RevokingWriter {
    fn finish(self: Box<Self>) -> Result<ArtifactReceipt, StoreError> {
        let receipt = self.writer.finish()?;
        let update = (|| -> Result<(), CraftError> {
            let control = control()?;
            let store = ProfileStore::open(&self.root, &control)?;
            store.set_review_policy(
                "factory",
                ReviewPolicyUpdate {
                    target: ResourceRef {
                        resource_type: "space".into(),
                        id: "100".into(),
                        ..target()
                    },
                    expected_actor: "actor-one".into(),
                    reviewers: vec!["human-one".into()],
                    excluded_actors: vec![],
                    quorum: 1,
                    enabled: false,
                    expected_revision: Some(1),
                },
                true,
                &control,
                |_| Ok(()),
            )?;
            Ok(())
        })();
        update.map_err(|_| StoreError::OutcomeUnknown)?;
        Ok(receipt)
    }
}
impl ArtifactStore for RevokeAfterPublish<'_> {
    fn begin(&self, spec: ArtifactSpec) -> Result<Box<dyn ArtifactWriter>, StoreError> {
        Ok(Box::new(RevokingWriter {
            writer: self.store.begin(spec)?,
            root: self.root.clone(),
        }))
    }
    fn receipt(&self, name: &str) -> Result<Option<ArtifactReceipt>, StoreError> {
        self.store.receipt(name)
    }
    fn read(&self, name: &str, max: u64) -> Result<Option<ArtifactBlob>, StoreError> {
        self.store.read(name, max)
    }
}
#[test]
fn policy_changes_after_native_reads_and_local_publication_keep_honest_effect_receipts()
-> TestResult {
    for mode in ["after_native", "after_publish"] {
        let setup = Setup::new()?;
        let jira_grant = configure(&setup, true)?;
        let (plan, design) = sources(false, false)?;
        let mut replies = vec![identity(), jira_identity()];
        replies.extend(reads("prepared", &plan, &design, false)?);
        let count = replies.len();
        let product = TlsServer::start(Reply::Sequence(replies))?;
        let issuer =
            issuer("read:confluence-user read:page:confluence read:jira-user read:jira-work")?;
        let clock = Arc::new(CapturedPolicyClock {
            base: ManualClock::new(1000),
            root: setup.root.clone(),
            counter: product.request_counter(),
            threshold: count,
            delay: 0,
            hits: std::sync::atomic::AtomicUsize::new(0),
            armed: std::sync::atomic::AtomicBool::new(false),
            changed: std::sync::atomic::AtomicBool::new(false),
        });
        let fixture = setup.fixture(&issuer, &product, clock.clone())?;
        for (grant, tenant) in [
            (&setup.grant, target().tenant_ref()),
            (&jira_grant, jira_target().tenant_ref()),
        ] {
            let locked = fixture.lock(grant, &control()?)?;
            fixture.client().provision_grant(
                &locked,
                fixture.service_credentials()?,
                &[tenant],
                Some("actor-one"),
            )?;
        }
        clock.base.advance(Duration::from_secs(3600))?;
        if mode == "after_native" {
            clock.armed.store(true, Ordering::SeqCst);
        }
        let artifacts =
            NativeArtifacts::open(&setup.root.join("reviews"), 32 * 1024 * 1024, &control()?)?;
        let revoking = RevokeAfterPublish {
            store: &artifacts,
            root: setup.root.clone(),
        };
        let selected: &dyn ArtifactStore = if mode == "after_publish" {
            &revoking
        } else {
            &artifacts
        };
        let (data, error) = prepare_native(
            args(false)?,
            "factory",
            "policy-bound-preparation",
            BindingEnvironment {
                store: &setup.store,
                credentials: &fixture,
                client: fixture.client(),
                tls: TlsPolicy::AdditionalCaBundle(product.root()),
                control: control()?,
            },
            selected,
        );
        assert!(error.is_some());
        assert!(!data.prepared);
        assert!(data.statement.is_none());
        assert_eq!(data.credential_cache_updates.len(), 2);
        let state = if mode == "after_publish" {
            craft_core::contracts::RequestState::Confirmed
        } else {
            craft_core::contracts::RequestState::NotSent
        };
        assert_eq!(data.artifact_write_state, state);
        assert_eq!(error.ok_or("policy failure")?.details.write_state, state);
        assert_eq!(data.artifact.is_some(), mode == "after_publish");
        assert_eq!(
            artifacts.receipt(&data.artifact_name)?.is_some(),
            mode == "after_publish"
        );
        if mode == "after_native" {
            assert!(clock.changed.load(Ordering::SeqCst));
        }
        assert_eq!(product.finish()?.len(), count);
        assert_eq!(issuer.finish()?.len(), 4);
        setup.no_secrets()?;
    }
    Ok(())
}
