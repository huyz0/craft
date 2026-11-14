use std::collections::VecDeque;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use craft_core::contracts::{CanonicalRequest, EffectClass, GrantRef, ResourceRef, Selection};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::{
    NativeCall, NativeResponse, OperationContext, OperationHandler, PreparedOperation,
};
use craft_core::registry::{
    OperationDescriptor, ReconciliationClass, Registry, RetryClass, response_schema, schema_for,
};
use craft_core::storage::Clock;
use craft_provider_jira::{
    create_metadata::{self, CreateInput, JiraMetadata, MetadataPrincipal},
    created_issue,
};
use serde_json::{Value, json};

fn subtask_host(parent: Value) -> Context {
    let mut values = fields();
    values.push(field("parent", "issuelink", true));
    values.push(field("assignee", "user", false));
    let mut host = context(values);
    host.responses[1].body["issueTypes"][0]["subtask"] = json!(true);
    host.responses.push_back(response(parent));
    host
}
fn parent() -> Value {
    json!({"id":"20000","key":"ED-1","self":"https://untrusted.invalid/parent","fields":{"project":{"id":"10000"},"issuetype":{"id":"10002","subtask":false}}})
}
fn account_host(body: Value) -> Context {
    Context {
        responses: vec![response(body)].into(),
        calls: vec![],
        cancel_after: None,
    }
}

#[test]
fn qualified_parent_and_exact_active_assignee_generate_only_bound_id_objects()
-> Result<(), Box<dyn std::error::Error>> {
    let clock = Arc::new(TestClock::default());
    let registry = registry(clock.clone())?;
    let read = operation(
        &registry,
        create_metadata::COMMAND,
        json!({"issue_type_id":"10001","parent_issue_id":"20000"}),
    )?;
    let mut host = subtask_host(parent());
    let proof = create_metadata::read_context(&read, &mut host, principal(), clock.clone())?;
    assert_eq!(
        proof.summary().parent.as_ref().ok_or("Missing parent")?.id,
        "20000"
    );
    assert_eq!(host.calls.len(), 4);
    assert_eq!(host.calls[3].route, "metadata-parent");
    assert_eq!(host.calls[3].query["fields"], "project,issuetype");
    assert_eq!(host.calls[3].query["updateHistory"], "false");
    assert_eq!(host.calls[3].query["failFast"], "true");
    let account = operation(
        &registry,
        create_metadata::ASSIGNEE_COMMAND,
        json!({"account_id":"account-two"}),
    )?;
    let mut users = account_host(
        json!([{"accountId":"account-two","active":true,"emailAddress":"private-email-sentinel","displayName":"private-name-sentinel","self":"https://untrusted.invalid/user"}]),
    );
    let assignee = create_metadata::read_assignable(&account, &mut users, principal(), clock)?;
    assert_eq!(users.calls.len(), 1);
    assert_eq!(users.calls[0].query["project"], "10000");
    assert_eq!(users.calls[0].query["accountId"], "account-two");
    assert!(!users.calls[0].query.contains_key("query"));
    let encoded = serde_json::to_string(assignee.summary())?;
    assert!(!encoded.contains("private-"));
    assert!(!assignee.summary().mutation_authorized);
    let create = operation(
        &registry,
        created_issue::COMMAND,
        json!({"summary":"Subtask","parent_issue_id":"20000","assignee_account_id":"account-two"}),
    )?;
    let frozen = proof.prepare_create_with_assignee(&create, &principal(), Some(&assignee))?;
    frozen.verify(&create, &principal())?;
    let body = frozen.call().body.as_ref().ok_or("Missing body")?;
    assert_eq!(body["fields"]["parent"], json!({"id":"20000"}));
    assert_eq!(
        body["fields"]["assignee"],
        json!({"accountId":"account-two"})
    );
    assert_eq!(
        registry
            .descriptor(create_metadata::ASSIGNEE_COMMAND)?
            .scope_alternatives,
        vec![vec!["read:jira-user".to_string()]]
    );
    assert_eq!(
        registry
            .descriptor(create_metadata::COMMAND)?
            .scope_alternatives,
        vec![vec!["read:jira-work".to_string()]]
    );
    Ok(())
}

#[test]
fn parent_identity_project_subtask_and_visibility_failures_never_return_a_proof()
-> Result<(), Box<dyn std::error::Error>> {
    for kind in [
        "id",
        "project",
        "nested_subtask",
        "missing_type",
        "permission",
    ] {
        let clock = Arc::new(TestClock::default());
        let registry = registry(clock.clone())?;
        let read = operation(
            &registry,
            create_metadata::COMMAND,
            json!({"issue_type_id":"10001","parent_issue_id":"20000"}),
        )?;
        let mut value = parent();
        match kind {
            "id" => value["id"] = json!("20001"),
            "project" => value["fields"]["project"]["id"] = json!("10099"),
            "nested_subtask" => value["fields"]["issuetype"]["subtask"] = json!(true),
            "missing_type" => value["fields"]["issuetype"] = Value::Null,
            _ => {}
        }
        let mut host = subtask_host(value);
        if kind == "permission" {
            host.responses[3].status = 403;
            host.responses[3].body = json!({"errorMessages":["private-parent-sentinel"]});
        }
        let error = create_metadata::read_context(&read, &mut host, principal(), clock)
            .err()
            .ok_or("Expected parent refusal")?;
        assert!(!error.retryable);
        assert!(!error.message.contains("private-parent-sentinel"));
        assert_eq!(host.calls.len(), 4);
    }
    Ok(())
}

#[test]
fn subtask_parent_input_must_match_the_qualified_parent_and_cannot_clear_it()
-> Result<(), Box<dyn std::error::Error>> {
    let clock = Arc::new(TestClock::default());
    let registry = registry(clock.clone())?;
    let read = operation(
        &registry,
        create_metadata::COMMAND,
        json!({"issue_type_id":"10001","parent_issue_id":"20000"}),
    )?;
    let mut host = subtask_host(parent());
    let proof = create_metadata::read_context(&read, &mut host, principal(), clock)?;
    for input in [
        json!({"summary":"Example"}),
        json!({"summary":"Example","parent_issue_id":null}),
        json!({"summary":"Example","parent_issue_id":"20001"}),
    ] {
        let create = operation(&registry, created_issue::COMMAND, input)?;
        assert!(proof.prepare_create(&create, &principal()).is_err());
    }
    let create = operation(
        &registry,
        created_issue::COMMAND,
        json!({"summary":"Example","parent_issue_id":"20000"}),
    )?;
    assert!(proof.prepare_create(&create, &principal()).is_ok());
    assert_eq!(host.calls.len(), 4);
    Ok(())
}

#[test]
fn assignable_lookup_requires_one_exact_active_account_and_never_falls_back_to_names()
-> Result<(), Box<dyn std::error::Error>> {
    let clock = Arc::new(TestClock::default());
    let registry = registry(clock.clone())?;
    let read = operation(
        &registry,
        create_metadata::ASSIGNEE_COMMAND,
        json!({"account_id":"account-two"}),
    )?;
    for body in [
        json!([]),
        json!([{"accountId":"account-other","active":true}]),
        json!([{"accountId":"account-two","active":false}]),
        json!([{"accountId":"account-two"}]),
        json!([{"accountId":"account-two","active":true},{"accountId":"account-two","active":true}]),
        json!([{"displayName":"account-two","emailAddress":"account-two","active":true}]),
    ] {
        let mut host = account_host(body);
        let error = create_metadata::read_assignable(&read, &mut host, principal(), clock.clone())
            .err()
            .ok_or("Expected account refusal")?;
        assert!(!error.retryable);
        assert_eq!(host.calls.len(), 1);
        assert_eq!(host.calls[0].query["maxResults"], "2");
    }
    for id in ["-1", "unassigned", " bad ", "a?token=secret"] {
        assert!(
            operation(
                &registry,
                create_metadata::ASSIGNEE_COMMAND,
                json!({"account_id":id})
            )
            .is_err()
        );
    }
    for status in [401, 403, 429, 500] {
        let mut host = account_host(json!({"secret":"private-user-error"}));
        host.responses[0].status = status;
        let error = create_metadata::read_assignable(&read, &mut host, principal(), clock.clone())
            .err()
            .ok_or("Expected failed lookup")?;
        assert!(!error.message.contains("private-user-error"));
        assert_eq!(host.calls.len(), 1);
    }
    Ok(())
}

#[test]
fn account_proof_cannot_cross_project_principal_account_or_its_own_freshness_window()
-> Result<(), Box<dyn std::error::Error>> {
    let clock = Arc::new(TestClock::default());
    let account_clock = Arc::new(TestClock::default());
    let registry = registry(clock.clone())?;
    let read = operation(
        &registry,
        create_metadata::COMMAND,
        json!({"issue_type_id":"10001"}),
    )?;
    let mut values = fields();
    values.push(field("assignee", "user", false));
    let mut host = context(values);
    let metadata = create_metadata::read_context(&read, &mut host, principal(), clock)?;
    let lookup = operation(
        &registry,
        create_metadata::ASSIGNEE_COMMAND,
        json!({"account_id":"account-two"}),
    )?;
    let mut users = account_host(json!([{"accountId":"account-two","active":true}]));
    let assignee =
        create_metadata::read_assignable(&lookup, &mut users, principal(), account_clock.clone())?;
    let create = operation(
        &registry,
        created_issue::COMMAND,
        json!({"summary":"Example","assignee_account_id":"account-two"}),
    )?;
    assert!(metadata.prepare_create(&create, &principal()).is_err());
    let frozen = metadata.prepare_create_with_assignee(&create, &principal(), Some(&assignee))?;
    let changed = operation(
        &registry,
        created_issue::COMMAND,
        json!({"summary":"Example","assignee_account_id":"account-other"}),
    )?;
    assert!(
        metadata
            .prepare_create_with_assignee(&changed, &principal(), Some(&assignee))
            .is_err()
    );
    let mut other = principal();
    other.actor = "account-other".into();
    assert!(
        metadata
            .prepare_create_with_assignee(&create, &other, Some(&assignee))
            .is_err()
    );
    let mut request = lookup.request().clone();
    request.target.id = "10099".into();
    let other_lookup = registry.prepare(request)?;
    let mut users = account_host(json!([{"accountId":"account-two","active":true}]));
    let other_proof = create_metadata::read_assignable(
        &other_lookup,
        &mut users,
        principal(),
        account_clock.clone(),
    )?;
    assert!(
        metadata
            .prepare_create_with_assignee(&create, &principal(), Some(&other_proof))
            .is_err()
    );
    account_clock.mono.store(60_000, Ordering::SeqCst);
    assert_eq!(
        frozen
            .verify(&create, &principal())
            .err()
            .ok_or("Expected account expiry")?
            .code,
        ErrorCode::VersionConflict
    );
    Ok(())
}

#[test]
fn explicit_unassignment_readonly_and_metadata_account_constraints_stay_distinct()
-> Result<(), Box<dyn std::error::Error>> {
    let clock = Arc::new(TestClock::default());
    let registry = registry(clock.clone())?;
    let read = operation(
        &registry,
        create_metadata::COMMAND,
        json!({"issue_type_id":"10001"}),
    )?;
    let lookup = operation(
        &registry,
        create_metadata::ASSIGNEE_COMMAND,
        json!({"account_id":"account-two"}),
    )?;
    let mut users = account_host(json!([{"accountId":"account-two","active":true}]));
    let assignee =
        create_metadata::read_assignable(&lookup, &mut users, principal(), clock.clone())?;
    for readonly in [false, true] {
        let mut values = fields();
        let mut user = field("assignee", "user", false);
        if readonly {
            user["operations"] = json!([]);
        } else {
            user["allowedValues"] = json!([{"accountId":"account-other"}]);
        }
        values.push(user);
        let mut host = context(values);
        let proof = create_metadata::read_context(&read, &mut host, principal(), clock.clone())?;
        let create = operation(
            &registry,
            created_issue::COMMAND,
            json!({"summary":"Example","assignee_account_id":"account-two"}),
        )?;
        assert!(
            proof
                .prepare_create_with_assignee(&create, &principal(), Some(&assignee))
                .is_err()
        );
        let clear = operation(
            &registry,
            created_issue::COMMAND,
            json!({"summary":"Example","assignee_account_id":null}),
        )?;
        assert_eq!(
            proof
                .prepare_create_with_assignee(&clear, &principal(), Some(&assignee))
                .err()
                .ok_or("Expected unassignment policy prerequisite")?
                .code,
            ErrorCode::UnsupportedCapability
        );
    }
    Ok(())
}

#[test]
fn explicit_nulls_and_external_schema_keys_cannot_bypass_native_field_validation()
-> Result<(), Box<dyn std::error::Error>> {
    let clock = Arc::new(TestClock::default());
    let registry = registry(clock.clone())?;
    let read = operation(
        &registry,
        create_metadata::COMMAND,
        json!({"issue_type_id":"10001"}),
    )?;
    let mut host = context(fields());
    let proof = create_metadata::read_context(&read, &mut host, principal(), clock.clone())?;
    for input in [
        json!({"summary":"Example","description":null}),
        json!({"summary":"Example","labels":null}),
        json!({"summary":"Example","labels":["duplicate","duplicate"]}),
        json!({"summary":"Example","labels":["contains space"]}),
    ] {
        let create = operation(&registry, created_issue::COMMAND, input)?;
        assert!(proof.prepare_create(&create, &principal()).is_err());
    }
    let mut values = fields();
    values[2]["schema"]["$ref"] = json!("https://untrusted.invalid/schema");
    let mut host = context(values);
    assert!(create_metadata::read_context(&read, &mut host, principal(), clock).is_err());
    assert_eq!(host.calls.len(), 3);
    Ok(())
}

#[test]
fn changing_totals_and_page_budget_never_produce_partial_metadata_authority()
-> Result<(), Box<dyn std::error::Error>> {
    let clock = Arc::new(TestClock::default());
    let registry = registry(clock.clone())?;
    let read = operation(
        &registry,
        create_metadata::COMMAND,
        json!({"issue_type_id":"10001"}),
    )?;
    let mut values = fields();
    for index in 0..50 {
        values.push(custom(
            &format!("customfield_{}", 1000 + index),
            "textfield",
            "string",
            false,
        ));
    }
    let mut host = context(values);
    host.responses[3].body["total"] = json!(56);
    assert!(create_metadata::read_context(&read, &mut host, principal(), clock.clone()).is_err());
    assert_eq!(host.calls.len(), 4);
    let read = operation(
        &registry,
        create_metadata::COMMAND,
        json!({"issue_type_id":"10001","page_size":1}),
    )?;
    let mut host = context(fields());
    host.responses.truncate(2);
    host.responses[1].body["maxResults"] = json!(1);
    for offset in 0..41 {
        host.responses.push_back(response(json!({"startAt":offset,"maxResults":1,"total":41,"fields":[field("summary","string",true)]})));
    }
    assert!(create_metadata::read_context(&read, &mut host, principal(), clock).is_err());
    assert_eq!(host.calls.len(), 42);
    Ok(())
}

#[derive(Default)]
struct TestClock {
    wall: AtomicU64,
    mono: AtomicU64,
}
impl Clock for TestClock {
    fn unix_millis(&self) -> u64 {
        self.wall.load(Ordering::SeqCst)
    }
    fn monotonic_millis(&self) -> u64 {
        self.mono.load(Ordering::SeqCst)
    }
}
struct Context {
    responses: VecDeque<NativeResponse>,
    calls: Vec<NativeCall>,
    cancel_after: Option<usize>,
}
impl OperationContext for Context {
    fn checkpoint(&self) -> Result<(), CraftError> {
        if self
            .cancel_after
            .is_some_and(|count| self.calls.len() >= count)
        {
            Err(CraftError::new(ErrorCode::Cancelled, "Cancelled fixture."))
        } else {
            Ok(())
        }
    }
    fn invoke(&mut self, call: &NativeCall) -> Result<NativeResponse, CraftError> {
        self.calls.push(call.clone());
        self.responses
            .pop_front()
            .ok_or_else(|| CraftError::new(ErrorCode::Internal, "Unexpected metadata request."))
    }
}
fn target() -> ResourceRef {
    ResourceRef {
        provider: "jira".into(),
        deployment: "cloud".into(),
        tenant: "00000000-1111-2222-3333-444444444444".into(),
        resource_type: "project".into(),
        id: "10000".into(),
    }
}
fn principal() -> MetadataPrincipal {
    MetadataPrincipal {
        grant: GrantRef {
            id: "gr_00000000000000000000000000000000".into(),
        },
        actor: "account-one".into(),
    }
}
fn field(id: &str, kind: &str, required: bool) -> Value {
    json!({"fieldId":id,"key":id,"name":id,"required":required,"hasDefaultValue":false,"operations":["set"],"schema":{"type":kind,"system":id}})
}
fn custom(id: &str, family: &str, kind: &str, required: bool) -> Value {
    json!({"fieldId":id,"key":id,"name":"Custom field","required":required,"hasDefaultValue":false,"operations":["set"],"schema":{"type":kind,"custom":format!("com.atlassian.jira.plugin.system.customfieldtypes:{family}"),"customId":id.strip_prefix("customfield_").and_then(|id|id.parse::<u64>().ok())}})
}
fn fields() -> Vec<Value> {
    vec![
        field("project", "project", true),
        field("issuetype", "issuetype", true),
        field("summary", "string", true),
        field("description", "string", false),
        {
            let mut value = field("labels", "array", false);
            value["schema"]["items"] = json!("string");
            value
        },
    ]
}
fn response(body: Value) -> NativeResponse {
    NativeResponse {
        status: 200,
        body,
        request_id: None,
        retry_after: None,
        link_headers: None,
    }
}
fn context(fields: Vec<Value>) -> Context {
    let mut responses = vec![
        response(json!({"id":"10000","key":"ED","self":"https://untrusted.invalid/ignore"})),
        response(
            json!({"startAt":0,"maxResults":50,"total":1,"issueTypes":[{"id":"10001","name":"Task","subtask":false,"self":"https://untrusted.invalid/ignore"}]}),
        ),
    ];
    for (start, chunk) in fields.chunks(50).enumerate() {
        responses.push(response(
            json!({"startAt":start*50,"maxResults":50,"total":fields.len(),"fields":chunk}),
        ));
    }
    Context {
        responses: responses.into(),
        calls: vec![],
        cancel_after: None,
    }
}
fn registry(clock: Arc<TestClock>) -> Result<Registry, CraftError> {
    let mut registry = Registry::default();
    registry.register_provider(&JiraMetadata::new(clock))?;
    registry.register_typed_handler::<CreateInput>(
        OperationDescriptor {
            id: created_issue::COMMAND.into(),
            provider: "jira".into(),
            contract_version: "1.0".into(),
            api_revision: "fixture".into(),
            description: "Frozen create fixture.".into(),
            effect: EffectClass::Write,
            input_schema: schema_for::<CreateInput>()?,
            response_schema: response_schema::<Value>()?,
            example: json!({"issue_type_id":"10001","summary":"Example"}),
            response_example: json!(craft_core::contracts::Envelope::success(
                created_issue::COMMAND,
                "example".into(),
                json!({})
            )),
            projection: None,
            scope_alternatives: vec![vec!["write:jira-work".into()]],
            permissions: vec![],
            tenant_conditions: vec![],
            routes: vec![],
            retry_class: RetryClass::ReconcileBeforeReplay,
            reconciliation: ReconciliationClass::Correlation,
        },
        Arc::new(Handler),
    )?;
    Ok(registry)
}
struct Handler;
impl OperationHandler for Handler {
    fn validate(&self, _: &CanonicalRequest) -> Result<(), CraftError> {
        Ok(())
    }
    fn execute(
        &self,
        _: &PreparedOperation,
        _: &mut dyn OperationContext,
    ) -> Result<Value, CraftError> {
        Err(CraftError::new(
            ErrorCode::UnsupportedCapability,
            "Not enabled.",
        ))
    }
}
fn operation(
    registry: &Registry,
    command: &str,
    mut input: Value,
) -> Result<PreparedOperation, CraftError> {
    if command == created_issue::COMMAND {
        input
            .as_object_mut()
            .ok_or_else(|| CraftError::field("/input", "expected_object"))?
            .entry("issue_type_id")
            .or_insert(json!("10001"));
    }
    registry.prepare(CanonicalRequest {
        command: command.into(),
        target: target(),
        input,
        selection: Selection::default(),
        preconditions: None,
    })
}

#[test]
fn explicit_issue_type_is_required_and_cannot_be_replaced_by_metadata()
-> Result<(), Box<dyn std::error::Error>> {
    let clock = Arc::new(TestClock::default());
    let registry = registry(clock.clone())?;
    assert!(
        registry
            .prepare(CanonicalRequest {
                command: created_issue::COMMAND.into(),
                target: target(),
                input: json!({"summary":"Example"}),
                selection: Selection::default(),
                preconditions: None
            })
            .is_err()
    );
    let mut host = context(fields());
    let read = operation(
        &registry,
        create_metadata::COMMAND,
        json!({"issue_type_id":"10001"}),
    )?;
    let metadata = create_metadata::read_context(&read, &mut host, principal(), clock)?;
    let wrong = operation(
        &registry,
        created_issue::COMMAND,
        json!({"issue_type_id":"10002","summary":"Example"}),
    )?;
    assert!(metadata.prepare_create(&wrong, &principal()).is_err());
    assert_eq!(host.calls.len(), 3);
    Ok(())
}

#[test]
fn complete_metadata_binds_target_principal_and_compiles_literal_adf_and_option_ids()
-> Result<(), Box<dyn std::error::Error>> {
    let clock = Arc::new(TestClock::default());
    let registry = registry(clock.clone())?;
    let mut values = fields();
    let mut select = custom("customfield_101", "select", "option", true);
    select["allowedValues"] =
        json!([{"id":"201","value":"Choice","self":"https://untrusted.invalid/option"}]);
    values.push(select);
    values.push(custom("customfield_102", "textarea", "string", false));
    let mut host = context(values);
    let read = operation(
        &registry,
        create_metadata::COMMAND,
        json!({"issue_type_id":"10001"}),
    )?;
    let proof = create_metadata::read_context(&read, &mut host, principal(), clock)?;
    assert!(proof.summary().complete);
    assert!(!proof.summary().mutation_authorized);
    assert_eq!(host.calls.len(), 3);
    assert_eq!(host.calls[0].route, "metadata-project");
    assert_eq!(host.calls[1].query["startAt"], "0");
    assert!(host.calls.iter().all(|call| call.body.is_none()));
    let create = operation(
        &registry,
        created_issue::COMMAND,
        json!({"summary":"Unicode 世界","description":"<script>literal</script>","labels":["valid"],"custom_fields":{"customfield_101":{"id":"201"},"customfield_102":"Line one\nLine two"}}),
    )?;
    let frozen = proof.prepare_create(&create, &principal())?;
    frozen.verify(&create, &principal())?;
    let body = frozen.call().body.as_ref().ok_or("Missing body")?;
    assert_eq!(body["fields"]["project"]["id"], "10000");
    assert_eq!(body["fields"]["issuetype"]["id"], "10001");
    assert_eq!(
        body["fields"]["description"]["content"][0]["content"][0]["text"],
        "<script>literal</script>"
    );
    assert_eq!(body["fields"]["customfield_101"], json!({"id":"201"}));
    assert_eq!(body["fields"]["customfield_102"]["type"], "doc");
    assert!(frozen.call().path_parameters.is_empty());
    Ok(())
}

#[test]
fn required_defaults_unknown_readonly_and_invalid_options_fail_before_any_mutation()
-> Result<(), Box<dyn std::error::Error>> {
    let clock = Arc::new(TestClock::default());
    let registry = registry(clock.clone())?;
    let mut values = fields();
    let mut select = custom("customfield_101", "select", "option", true);
    select["allowedValues"] = json!([{"id":"201"}]);
    values.push(select);
    let mut unsupported = custom("customfield_102", "third-party", "string", true);
    unsupported["hasDefaultValue"] = json!(true);
    values.push(unsupported);
    let mut readonly = custom("customfield_103", "textfield", "string", false);
    readonly["operations"] = json!([]);
    values.push(readonly);
    let mut host = context(values);
    let read = operation(
        &registry,
        create_metadata::COMMAND,
        json!({"issue_type_id":"10001"}),
    )?;
    let proof = create_metadata::read_context(&read, &mut host, principal(), clock)?;
    for input in [
        json!({"summary":"Example"}),
        json!({"summary":"Example","custom_fields":{"customfield_101":{"value":"Choice"}}}),
        json!({"summary":"Example","custom_fields":{"customfield_101":{"id":"202"}}}),
        json!({"summary":"Example","custom_fields":{"customfield_101":{"id":"201","extra":true}}}),
        json!({"summary":"Example","custom_fields":{"customfield_101":{"id":"201"},"customfield_103":"read only"}}),
        json!({"summary":"Example","custom_fields":{"status":"Done"}}),
    ] {
        let create = operation(&registry, created_issue::COMMAND, input)?;
        assert!(proof.prepare_create(&create, &principal()).is_err());
    }
    let create = operation(
        &registry,
        created_issue::COMMAND,
        json!({"summary":"Example","custom_fields":{"customfield_101":{"id":"201"}}}),
    )?;
    assert!(proof.prepare_create(&create, &principal()).is_ok());
    assert_eq!(host.calls.len(), 3);
    assert!(
        operation(
            &registry,
            created_issue::COMMAND,
            json!({"summary":"Example","status":"Done"})
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn stale_or_wrong_principal_project_operation_and_subtask_never_reuse_context()
-> Result<(), Box<dyn std::error::Error>> {
    let clock = Arc::new(TestClock::default());
    clock.wall.store(100, Ordering::SeqCst);
    clock.mono.store(100, Ordering::SeqCst);
    let registry = registry(clock.clone())?;
    let read = operation(
        &registry,
        create_metadata::COMMAND,
        json!({"issue_type_id":"10001"}),
    )?;
    let mut host = context(fields());
    let proof = create_metadata::read_context(&read, &mut host, principal(), clock.clone())?;
    let create = operation(
        &registry,
        created_issue::COMMAND,
        json!({"summary":"Example"}),
    )?;
    let frozen = proof.prepare_create(&create, &principal())?;
    let mut other = principal();
    other.actor = "account-two".into();
    assert!(proof.prepare_create(&create, &other).is_err());
    assert!(frozen.verify(&create, &other).is_err());
    let changed = operation(
        &registry,
        created_issue::COMMAND,
        json!({"summary":"Changed"}),
    )?;
    assert!(frozen.verify(&changed, &principal()).is_err());
    clock.wall.store(99, Ordering::SeqCst);
    assert!(proof.prepare_create(&create, &principal()).is_err());
    clock.wall.store(100, Ordering::SeqCst);
    clock.mono.store(60_100, Ordering::SeqCst);
    assert!(frozen.verify(&create, &principal()).is_err());
    clock.mono.store(100, Ordering::SeqCst);
    let mut host = context(fields());
    host.responses[1].body["issueTypes"][0]["subtask"] = json!(true);
    let subtask = create_metadata::read_context(&read, &mut host, principal(), clock)?;
    assert_eq!(
        subtask
            .prepare_create(&create, &principal())
            .err()
            .ok_or("Expected parent prerequisite")?
            .code,
        ErrorCode::UnsupportedCapability
    );
    Ok(())
}

#[test]
fn pagination_rejects_incomplete_inconsistent_duplicate_and_untrusted_alias_pages()
-> Result<(), Box<dyn std::error::Error>> {
    for kind in [
        "offset",
        "total",
        "empty",
        "duplicate",
        "aliases",
        "bound",
        "missing_type",
        "wrong_project",
    ] {
        let clock = Arc::new(TestClock::default());
        let registry = registry(clock.clone())?;
        let read = operation(
            &registry,
            create_metadata::COMMAND,
            json!({"issue_type_id":"10001"}),
        )?;
        let mut host = context(fields());
        match kind {
            "offset" => host.responses[2].body["startAt"] = json!(1),
            "total" => host.responses[2].body["total"] = json!(1),
            "empty" => {
                host.responses[2].body["fields"] = json!([]);
                host.responses[2].body["total"] = json!(1);
            }
            "duplicate" => {
                let first = host.responses[2].body["fields"][0].clone();
                host.responses[2].body["fields"][1] = first;
            }
            "aliases" => host.responses[2].body["results"] = json!([]),
            "bound" => host.responses[1].body["total"] = json!(2001),
            "missing_type" => host.responses[1].body["issueTypes"][0]["id"] = json!("999"),
            _ => host.responses[0].body["id"] = json!("999"),
        }
        assert!(
            create_metadata::read_context(&read, &mut host, principal(), clock).is_err(),
            "{kind}"
        );
        assert!(host.calls.len() <= 3);
    }
    Ok(())
}

#[test]
fn complete_multi_page_fields_and_registry_projection_keep_only_requested_metadata()
-> Result<(), Box<dyn std::error::Error>> {
    let clock = Arc::new(TestClock::default());
    let registry = registry(clock.clone())?;
    let mut values = fields();
    for index in 0..50 {
        values.push(custom(
            &format!("customfield_{}", 1000 + index),
            "textfield",
            "string",
            false,
        ));
    }
    let mut host = context(values.clone());
    let read = operation(
        &registry,
        create_metadata::COMMAND,
        json!({"issue_type_id":"10001"}),
    )?;
    let proof = create_metadata::read_context(&read, &mut host, principal(), clock)?;
    assert_eq!(proof.summary().fields.len(), 55);
    assert_eq!(host.calls[3].query["startAt"], "50");
    let mut host = context(values);
    let mut request = read.request().clone();
    request.selection.fields = vec!["fields.id".into()];
    let selected = registry.prepare(request)?;
    let data = registry.execute(&selected, &mut host)?;
    assert_eq!(data["complete"], true);
    assert_eq!(data["mutation_authorized"], false);
    assert!(data["fields"][0].get("schema").is_none());
    assert_eq!(data["fields"][0]["id"], "project");
    Ok(())
}

#[test]
fn metadata_errors_cancellation_and_wrong_operation_fail_without_followup_or_retry()
-> Result<(), Box<dyn std::error::Error>> {
    for status in [401, 403, 429, 500] {
        let clock = Arc::new(TestClock::default());
        let registry = registry(clock.clone())?;
        let read = operation(
            &registry,
            create_metadata::COMMAND,
            json!({"issue_type_id":"10001"}),
        )?;
        let mut host = context(fields());
        host.responses[0].status = status;
        host.responses[0].body = json!({"private":"remote-error-sentinel"});
        let error = create_metadata::read_context(&read, &mut host, principal(), clock)
            .err()
            .ok_or("Expected metadata refusal")?;
        assert_eq!(host.calls.len(), 1);
        assert!(!error.retryable);
        assert!(!error.message.contains("remote-error-sentinel"));
    }
    let clock = Arc::new(TestClock::default());
    let registry = registry(clock.clone())?;
    let mut host = context(fields());
    host.cancel_after = Some(1);
    let read = operation(
        &registry,
        create_metadata::COMMAND,
        json!({"issue_type_id":"10001"}),
    )?;
    assert_eq!(
        create_metadata::read_context(&read, &mut host, principal(), clock.clone())
            .err()
            .ok_or("Expected cancellation")?
            .code,
        ErrorCode::Cancelled
    );
    assert_eq!(host.calls.len(), 1);
    let create = operation(
        &registry,
        created_issue::COMMAND,
        json!({"summary":"Example"}),
    )?;
    assert!(create_metadata::read_context(&create, &mut host, principal(), clock).is_err());
    assert_eq!(host.calls.len(), 1);
    Ok(())
}
