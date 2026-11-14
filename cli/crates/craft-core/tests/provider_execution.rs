use std::collections::BTreeMap;
use std::sync::Arc;

use craft_core::contracts::{
    CanonicalRequest, EffectClass, Envelope, RequestState, ResourceRef, SCHEMA_VERSION, Selection,
};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::{
    HttpMethod, NativeCall, NativeResponse, NativeRoute, OperationContext, OperationHandler,
    PreparedOperation, RouteSegment,
};
use craft_core::registry::{
    OperationDescriptor, ProviderManifest, ProviderModule, ReconciliationClass, Registry,
    RetryClass, response_schema,
};
use serde_json::{Value, json};

#[test]
fn repeated_get_queries_require_explicit_disjoint_declarations_and_preserve_legacy_encoding()
-> Result<(), Box<dyn std::error::Error>> {
    let mut route = NativeRoute {
        id: "read".into(),
        method: HttpMethod::Get,
        path: vec![RouteSegment::Literal("records".into())],
        query_parameters: vec!["id".into(), "cursor".into()],
        continuation_parameters: vec!["cursor".into()],
        repeated_query_parameters: vec![],
        response_max_bytes: 4096,
    };
    let legacy = json!({"id":"read","method":"get","path":[{"kind":"literal","value":"records"}],"query_parameters":["id","cursor"],"continuation_parameters":["cursor"],"response_max_bytes":4096});
    assert_eq!(serde_json::to_value(&route)?, legacy);
    assert!(
        serde_json::from_value::<NativeRoute>(legacy)?
            .repeated_query_parameters
            .is_empty()
    );
    route.repeated_query_parameters = vec!["id".into()];
    route.validate()?;
    for mode in [
        "undeclared",
        "duplicate",
        "continuation",
        "post",
        "put",
        "patch",
        "delete",
        "credential",
    ] {
        let mut invalid = route.clone();
        match mode {
            "undeclared" => invalid.repeated_query_parameters = vec!["other".into()],
            "duplicate" => invalid.repeated_query_parameters.push("id".into()),
            "continuation" => invalid.repeated_query_parameters = vec!["cursor".into()],
            "post" => invalid.method = HttpMethod::Post,
            "put" => invalid.method = HttpMethod::Put,
            "patch" => invalid.method = HttpMethod::Patch,
            "delete" => invalid.method = HttpMethod::Delete,
            _ => {
                invalid.query_parameters.push("access-token".into());
                invalid.repeated_query_parameters = vec!["access-token".into()];
            }
        }
        assert!(invalid.validate().is_err(), "{mode}");
    }
    Ok(())
}

#[test]
fn repeated_query_values_enforce_name_pair_and_utf8_byte_bounds_without_coercion()
-> Result<(), CraftError> {
    let mut route = NativeRoute {
        id: "read".into(),
        method: HttpMethod::Get,
        path: vec![RouteSegment::Literal("records".into())],
        query_parameters: vec!["q".into(), "a".into(), "b".into(), "c".into()],
        continuation_parameters: vec![],
        repeated_query_parameters: vec!["a".into(), "b".into(), "c".into()],
        response_max_bytes: 4096,
    };
    let call = NativeCall {
        route: "read".into(),
        path_parameters: BTreeMap::new(),
        query: BTreeMap::from([("q".into(), "literal".into())]),
        repeated_query: BTreeMap::from([(
            "a".into(),
            vec!["".into(), "a,b & 文字".into(), "same".into(), "same".into()],
        )]),
        body: None,
    };
    route.validate_call(&call)?;
    let mut edge = call.clone();
    edge.repeated_query = BTreeMap::from([
        ("a".into(), vec!["x".into(); 100]),
        ("b".into(), vec!["x".into(); 100]),
        ("c".into(), vec!["x".into(); 55]),
    ]);
    route.validate_call(&edge)?;
    edge.repeated_query
        .get_mut("c")
        .ok_or_else(|| CraftError::field("/fixture", "missing_array"))?
        .push("extra".into());
    assert!(route.validate_call(&edge).is_err());
    for mode in [
        "scalar",
        "collision",
        "unknown",
        "empty",
        "count",
        "bytes",
        "post",
    ] {
        let mut invalid = call.clone();
        let mut declaration = route.clone();
        match mode {
            "scalar" => {
                invalid.repeated_query.clear();
                invalid.query.insert("a".into(), "x".into());
            }
            "collision" => {
                invalid.query.insert("a".into(), "x".into());
            }
            "unknown" => {
                invalid
                    .repeated_query
                    .insert("other".into(), vec!["x".into()]);
            }
            "empty" => {
                invalid.repeated_query.insert("a".into(), vec![]);
            }
            "count" => {
                invalid
                    .repeated_query
                    .insert("a".into(), vec!["x".into(); 101]);
            }
            "bytes" => {
                invalid
                    .repeated_query
                    .insert("a".into(), vec!["é".repeat(8193)]);
            }
            _ => declaration.method = HttpMethod::Post,
        }
        assert!(declaration.validate_call(&invalid).is_err(), "{mode}");
    }
    let mut exact = call.clone();
    exact
        .repeated_query
        .insert("a".into(), vec!["é".repeat(8192)]);
    route.validate_call(&exact)?;
    route.query_parameters = (0..33).map(|index| format!("q{index}")).collect();
    route.repeated_query_parameters.clear();
    let mut many = call;
    many.repeated_query.clear();
    many.query = route
        .query_parameters
        .iter()
        .map(|name| (name.clone(), "x".into()))
        .collect();
    assert!(route.validate_call(&many).is_err());
    Ok(())
}

#[test]
fn native_provider_parameter_names_keep_credential_queries_forbidden() -> Result<(), CraftError> {
    let mut route = NativeRoute {
        repeated_query_parameters: Vec::new(),
        id: "comment-read".into(),
        method: HttpMethod::Get,
        path: vec![
            RouteSegment::Literal("footer-comments".into()),
            RouteSegment::Parameter("comment-id".into()),
        ],
        query_parameters: vec!["body-format".into(), "include-properties".into()],
        continuation_parameters: Vec::new(),
        response_max_bytes: 16_384,
    };
    route.validate()?;
    route.validate_call(&NativeCall {
        repeated_query: std::collections::BTreeMap::new(),
        route: "comment-read".into(),
        path_parameters: BTreeMap::from([("comment-id".into(), "123".into())]),
        query: BTreeMap::from([("body-format".into(), "storage".into())]),
        body: None,
    })?;
    for name in [
        "access_token",
        "access-token",
        "refresh_token",
        "refresh-token",
        "client_secret",
        "client-secret",
        "Authorization",
    ] {
        route.query_parameters = vec![name.into()];
        assert!(route.validate().is_err());
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum Behavior {
    Read,
    UndeclaredRoute,
    ExtraParameter,
    BadResult,
}

struct SampleHandler(Behavior);

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SampleInput {
    id: String,
}

impl OperationHandler for SampleHandler {
    fn validate(&self, request: &CanonicalRequest) -> Result<(), CraftError> {
        if request.target.resource_type != "record" {
            return Err(CraftError::field(
                "/target/resource_type",
                "expected_record_target",
            ));
        }
        let input: SampleInput = serde_json::from_value(request.input.clone())
            .map_err(|_| CraftError::field("/id", "expected_record_identity"))?;
        if input.id != request.target.id {
            return Err(CraftError::field("/id", "target_identity_mismatch"));
        }
        Ok(())
    }

    fn execute(
        &self,
        operation: &PreparedOperation,
        context: &mut dyn OperationContext,
    ) -> Result<Value, CraftError> {
        let mut call = NativeCall {
            repeated_query: std::collections::BTreeMap::new(),
            route: "read".into(),
            path_parameters: BTreeMap::from([("id".into(), operation.request().target.id.clone())]),
            query: BTreeMap::new(),
            body: None,
        };
        if matches!(self.0, Behavior::UndeclaredRoute) {
            call.route = "other".into();
        }
        if matches!(self.0, Behavior::ExtraParameter) {
            call.path_parameters
                .insert("host".into(), "untrusted".into());
        }
        let response = context.invoke(&call)?;
        if matches!(self.0, Behavior::BadResult) {
            return Ok(json!(42));
        }
        Ok(response.body)
    }
}

struct SampleProvider(Behavior);

impl ProviderModule for SampleProvider {
    fn manifest(&self) -> ProviderManifest {
        ProviderManifest {
            id: "sample".into(),
            version: "0.1.0".into(),
            contract_version: SCHEMA_VERSION.into(),
            deployments: vec!["cloud".into()],
        }
    }

    fn register(&self, registry: &mut Registry) -> Result<(), CraftError> {
        registry
            .register_typed_handler::<SampleInput>(descriptor()?, Arc::new(SampleHandler(self.0)))
    }
}

fn descriptor() -> Result<OperationDescriptor, CraftError> {
    Ok(OperationDescriptor {
        id: "sample.read".into(),
        provider: "sample".into(),
        contract_version: SCHEMA_VERSION.into(),
        api_revision: "native-v1".into(),
        description: "Read a sample through its declared route.".into(),
        effect: EffectClass::Read,
        input_schema: json!({"type":"object","properties":{"id":{"type":"string"}},"required":["id"],"additionalProperties":false}),
        response_schema: response_schema::<String>()?,
        example: json!({"id":"record-one"}),
        response_example: serde_json::to_value(Envelope::success(
            "sample.read",
            "example".into(),
            json!("sample"),
        ))
        .map_err(|_| CraftError::new(ErrorCode::Internal, "Cannot encode example."))?,
        projection: None,
        scope_alternatives: vec![
            vec!["sample:read".into()],
            vec!["sample:all".into(), "tenant:read".into()],
        ],
        permissions: vec!["browse".into()],
        tenant_conditions: vec!["feature_available".into()],
        routes: vec![NativeRoute {
            repeated_query_parameters: Vec::new(),
            continuation_parameters: Vec::new(),
            id: "read".into(),
            method: HttpMethod::Get,
            path: vec![
                RouteSegment::Literal("records".into()),
                RouteSegment::Parameter("id".into()),
            ],
            query_parameters: vec!["fields".into()],
            response_max_bytes: 512,
        }],
        retry_class: RetryClass::SafeRead,
        reconciliation: ReconciliationClass::NotApplicable,
    })
}

fn request() -> CanonicalRequest {
    CanonicalRequest {
        command: "sample.read".into(),
        target: ResourceRef {
            provider: "sample".into(),
            deployment: "cloud".into(),
            tenant: "tenant-one".into(),
            resource_type: "record".into(),
            id: "record-one".into(),
        },
        input: json!({"id":"record-one"}),
        selection: Selection::default(),
        preconditions: None,
    }
}

#[derive(Default)]
struct RecordingHost {
    calls: Vec<String>,
    cancelled: bool,
    oversized: bool,
}

impl OperationContext for RecordingHost {
    fn checkpoint(&self) -> Result<(), CraftError> {
        if self.cancelled {
            return Err(CraftError::new(
                ErrorCode::Cancelled,
                "Operation cancelled.",
            ));
        }
        Ok(())
    }
    fn invoke(&mut self, call: &NativeCall) -> Result<NativeResponse, CraftError> {
        self.calls
            .push(format!("{}:{}", call.route, call.path_parameters["id"]));
        Ok(NativeResponse {
            retry_after: None,
            link_headers: None,
            status: 200,
            body: json!(if self.oversized {
                "x".repeat(513)
            } else {
                "sample".into()
            }),
            request_id: None,
        })
    }
}

#[test]
fn executes_a_compiled_provider_without_changing_core_dispatch()
-> Result<(), Box<dyn std::error::Error>> {
    let mut registry = Registry::default();
    registry.register_provider(&SampleProvider(Behavior::Read))?;
    assert_eq!(registry.providers().count(), 1);
    let operation = registry.prepare(request())?;
    assert_eq!(operation.effect(), EffectClass::Read);
    assert!(operation.digest().starts_with("sha256:"));
    let mut host = RecordingHost::default();
    assert_eq!(registry.execute(&operation, &mut host)?, "sample");
    assert_eq!(host.calls, ["read:record-one"]);
    assert!(registry.reconcile(&operation, &mut host).is_err());
    assert_eq!(host.calls.len(), 1);
    Ok(())
}

#[test]
fn rejects_local_target_and_route_errors_before_host_calls()
-> Result<(), Box<dyn std::error::Error>> {
    let mut registry = Registry::default();
    registry.register_provider(&SampleProvider(Behavior::Read))?;
    for changed in ["provider", "deployment", "resource_type", "input"] {
        let mut invalid = request();
        match changed {
            "provider" => invalid.target.provider = "other".into(),
            "deployment" => invalid.target.deployment = "server".into(),
            "resource_type" => invalid.target.resource_type = "space".into(),
            _ => invalid.input = json!({"id":"record-two"}),
        }
        assert!(registry.prepare(invalid).is_err());
    }
    for behavior in [Behavior::UndeclaredRoute, Behavior::ExtraParameter] {
        let mut registry = Registry::default();
        registry.register_provider(&SampleProvider(behavior))?;
        let operation = registry.prepare(request())?;
        let mut host = RecordingHost::default();
        let error = registry
            .execute(&operation, &mut host)
            .err()
            .ok_or("Expected route refusal")?;
        assert_eq!(error.code, ErrorCode::PolicyDenied);
        assert!(host.calls.is_empty());
    }
    Ok(())
}

#[test]
fn rejects_contract_drift_and_cancelled_execution_before_host_calls()
-> Result<(), Box<dyn std::error::Error>> {
    let mut registry = Registry::default();
    registry.register_provider(&SampleProvider(Behavior::Read))?;
    let operation = registry.prepare(request())?;
    let mut changed_registry = Registry::default();
    let mut changed = descriptor()?;
    changed.api_revision = "native-v2".into();
    changed_registry.register_handler(changed, Arc::new(SampleHandler(Behavior::Read)))?;
    let mut host = RecordingHost::default();
    assert_eq!(
        changed_registry
            .execute(&operation, &mut host)
            .err()
            .ok_or("Expected drift refusal")?
            .code,
        ErrorCode::VersionConflict
    );
    host.cancelled = true;
    assert_eq!(
        registry
            .execute(&operation, &mut host)
            .err()
            .ok_or("Expected cancellation")?
            .code,
        ErrorCode::Cancelled
    );
    assert!(host.calls.is_empty());
    Ok(())
}

#[test]
fn read_descriptors_cannot_hide_mutating_verbs_while_post_search_remains_valid()
-> Result<(), Box<dyn std::error::Error>> {
    for method in [HttpMethod::Put, HttpMethod::Patch, HttpMethod::Delete] {
        let mut route = descriptor()?;
        route.routes[0].method = method;
        let mut registry = Registry::default();
        assert!(
            registry
                .register_handler(route, Arc::new(SampleHandler(Behavior::Read)))
                .is_err()
        );
        assert_eq!(registry.descriptors().count(), 0);
    }
    let mut search = descriptor()?;
    search.routes[0].method = HttpMethod::Post;
    let mut registry = Registry::default();
    registry.register_handler(search, Arc::new(SampleHandler(Behavior::Read)))?;
    assert_eq!(registry.descriptors().count(), 1);
    Ok(())
}

#[test]
fn rejects_invalid_handler_data_and_bounds_native_responses()
-> Result<(), Box<dyn std::error::Error>> {
    for behavior in [Behavior::Read, Behavior::BadResult] {
        let mut registry = Registry::default();
        registry.register_provider(&SampleProvider(behavior))?;
        let operation = registry.prepare(request())?;
        let mut host = RecordingHost {
            oversized: matches!(behavior, Behavior::Read),
            ..RecordingHost::default()
        };
        let error = registry
            .execute(&operation, &mut host)
            .err()
            .ok_or("Expected result refusal")?;
        assert_eq!(error.details.write_state, RequestState::Failed);
        assert_eq!(host.calls.len(), 1);
    }
    Ok(())
}

#[test]
fn rejects_duplicate_provider_modules_and_invalid_route_contracts_atomically()
-> Result<(), Box<dyn std::error::Error>> {
    let mut registry = Registry::default();
    registry.register_provider(&SampleProvider(Behavior::Read))?;
    assert!(
        registry
            .register_provider(&SampleProvider(Behavior::Read))
            .is_err()
    );
    assert_eq!(registry.providers().count(), 1);
    assert_eq!(registry.descriptors().count(), 1);
    let mut invalid = descriptor()?;
    invalid.routes[0]
        .query_parameters
        .push("access_token".into());
    let mut empty = Registry::default();
    assert!(
        empty
            .register_handler(invalid, Arc::new(SampleHandler(Behavior::Read)))
            .is_err()
    );
    assert_eq!(empty.descriptors().count(), 0);
    Ok(())
}

#[test]
fn never_reports_not_sent_when_mutation_response_validation_fails()
-> Result<(), Box<dyn std::error::Error>> {
    for oversized in [false, true] {
        let mut registry = Registry::default();
        let mut mutation = descriptor()?;
        mutation.id = "sample.write".into();
        mutation.effect = EffectClass::Write;
        mutation.routes[0].method = HttpMethod::Post;
        mutation.retry_class = RetryClass::ReconcileBeforeReplay;
        registry.register_handler(mutation, Arc::new(SampleHandler(Behavior::BadResult)))?;
        let mut input = request();
        input.command = "sample.write".into();
        let operation = registry.prepare(input)?;
        let mut host = RecordingHost {
            oversized,
            ..RecordingHost::default()
        };
        let error = registry
            .execute(&operation, &mut host)
            .err()
            .ok_or("Expected uncertain result")?;
        assert_eq!(host.calls.len(), 1);
        assert_eq!(error.code, ErrorCode::WriteOutcomeUnknown);
        assert_eq!(error.details.write_state, RequestState::SentUnknown);
        assert!(!error.retryable);
    }
    Ok(())
}

#[test]
fn treats_native_post_searches_as_reads_by_declared_effect()
-> Result<(), Box<dyn std::error::Error>> {
    let mut registry = Registry::default();
    let mut search = descriptor()?;
    search.routes[0].method = HttpMethod::Post;
    registry.register_handler(search, Arc::new(SampleHandler(Behavior::Read)))?;
    let operation = registry.prepare(request())?;
    let mut host = RecordingHost {
        oversized: true,
        ..RecordingHost::default()
    };
    let error = registry
        .execute(&operation, &mut host)
        .err()
        .ok_or("Expected bounded read failure")?;
    assert_eq!(error.code, ErrorCode::RemoteUnavailable);
    assert_eq!(error.details.write_state, RequestState::Failed);
    assert_eq!(host.calls.len(), 1);
    Ok(())
}
