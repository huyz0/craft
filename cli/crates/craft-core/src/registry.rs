use std::collections::BTreeMap;
use std::sync::Arc;

use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::contracts::{CanonicalRequest, EffectClass, Envelope, validate_identity};
use crate::error::{CraftError, ErrorCode, FieldIssue};
use crate::execution::{
    CheckedContext, HttpMethod, NativeRoute, OperationContext, OperationHandler, PreparedOperation,
};
use crate::input::canonical_digest;
use crate::projection::{CompiledProjection, ProjectionContract};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProviderManifest {
    pub id: String,
    pub version: String,
    pub deployments: Vec<String>,
    pub contract_version: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RetryClass {
    LocalRead,
    SafeRead,
    Never,
    ReconcileBeforeReplay,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReconciliationClass {
    NotApplicable,
    KnownResource,
    Correlation,
}

/// A conjunction of scopes inside each alternative, with disjunction between alternatives.
pub type ScopeAlternatives = Vec<Vec<String>>;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OperationDescriptor {
    pub id: String,
    pub provider: String,
    pub contract_version: String,
    pub api_revision: String,
    pub description: String,
    pub effect: EffectClass,
    pub input_schema: Value,
    pub response_schema: Value,
    pub example: Value,
    pub response_example: Value,
    pub projection: Option<ProjectionContract>,
    pub scope_alternatives: ScopeAlternatives,
    pub permissions: Vec<String>,
    pub tenant_conditions: Vec<String>,
    pub routes: Vec<NativeRoute>,
    pub retry_class: RetryClass,
    pub reconciliation: ReconciliationClass,
}

pub trait ProviderModule {
    fn manifest(&self) -> ProviderManifest;
    fn register(&self, registry: &mut Registry) -> Result<(), CraftError>;
}

type TypedInputValidator = fn(&Value) -> Result<(), CraftError>;

struct RegisteredOperation {
    descriptor: OperationDescriptor,
    validator: jsonschema::Validator,
    response_validator: jsonschema::Validator,
    handler: Option<Arc<dyn OperationHandler>>,
    typed_validator: Option<TypedInputValidator>,
}

#[derive(Default)]
pub struct Registry {
    operations: BTreeMap<String, RegisteredOperation>,
    providers: BTreeMap<String, ProviderManifest>,
}

pub(crate) fn validate_local_references(value: &Value) -> Result<(), CraftError> {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                if matches!(key.as_str(), "$ref" | "$dynamicRef")
                    && value
                        .as_str()
                        .is_none_or(|reference| !reference.starts_with('#'))
                {
                    return Err(CraftError::new(
                        ErrorCode::Internal,
                        "Operation schemas must contain only bundled local references.",
                    ));
                }
                validate_local_references(value)?;
            }
        }
        Value::Array(array) => {
            for value in array {
                validate_local_references(value)?;
            }
        }
        _ => {}
    }
    Ok(())
}

pub fn schema_for<T: JsonSchema>() -> Result<Value, CraftError> {
    serde_json::to_value(schemars::schema_for!(T))
        .map_err(|_| CraftError::new(ErrorCode::Internal, "Cannot generate operation schema."))
}

pub fn response_schema<T: JsonSchema>() -> Result<Value, CraftError> {
    let mut envelope = schema_for::<Envelope>()?;
    let mut data = schema_for::<T>()?;
    if let Some(definitions) = data
        .as_object_mut()
        .and_then(|object| object.remove("$defs"))
    {
        let root = envelope
            .as_object_mut()
            .ok_or_else(|| CraftError::new(ErrorCode::Internal, "Invalid envelope schema."))?;
        let root_definitions = root.entry("$defs").or_insert_with(|| json!({}));
        let target = root_definitions
            .as_object_mut()
            .ok_or_else(|| CraftError::new(ErrorCode::Internal, "Invalid schema definitions."))?;
        if let Value::Object(definitions) = definitions {
            for (name, schema) in definitions {
                if target
                    .get(&name)
                    .is_some_and(|existing| existing != &schema)
                {
                    return Err(CraftError::new(
                        ErrorCode::Internal,
                        "Conflicting bundled schema definitions.",
                    ));
                }
                target.insert(name, schema);
            }
        }
    }
    envelope["properties"]["data"] = data;
    envelope["properties"]["error"] = json!({"$ref":"#/$defs/CraftError"});
    envelope["properties"]["schema_version"] = json!({"const":crate::contracts::SCHEMA_VERSION});
    envelope["oneOf"] = json!([
        {"properties":{"ok":{"const":true}},"required":["data"],"not":{"required":["error"]}},
        {"properties":{"ok":{"const":false}},"required":["error"]}
    ]);
    Ok(envelope)
}

impl Registry {
    pub fn register(&mut self, descriptor: OperationDescriptor) -> Result<(), CraftError> {
        if descriptor.contract_version != crate::contracts::SCHEMA_VERSION {
            return Err(CraftError::new(
                ErrorCode::Internal,
                "Unsupported operation contract version.",
            ));
        }
        validate_identity("/provider", &descriptor.provider)?;
        validate_identity("/api_revision", &descriptor.api_revision)?;
        let mut route_ids = std::collections::BTreeSet::new();
        for route in &descriptor.routes {
            route.validate()?;
            if matches!(descriptor.effect, EffectClass::Read)
                && matches!(
                    route.method,
                    HttpMethod::Put | HttpMethod::Patch | HttpMethod::Delete
                )
            {
                return Err(CraftError::new(
                    ErrorCode::Internal,
                    "Read operations cannot declare mutating HTTP methods.",
                ));
            }
            if !route_ids.insert(&route.id) {
                return Err(CraftError::new(
                    ErrorCode::Internal,
                    "Duplicate native route identity.",
                ));
            }
        }
        if descriptor.id.is_empty()
            || descriptor.id.len() > 128
            || !descriptor.id.chars().all(|character| {
                character.is_ascii_lowercase()
                    || character.is_ascii_digit()
                    || matches!(character, '.' | '-')
            })
        {
            return Err(CraftError::new(
                ErrorCode::Internal,
                "Invalid registered command identity.",
            ));
        }
        if self.operations.contains_key(&descriptor.id) {
            return Err(CraftError::new(
                ErrorCode::Internal,
                "Duplicate registered command.",
            ));
        }
        validate_local_references(&descriptor.input_schema)?;
        validate_local_references(&descriptor.response_schema)?;
        let validator = jsonschema::validator_for(&descriptor.input_schema)
            .map_err(|_| CraftError::new(ErrorCode::Internal, "Invalid command input schema."))?;
        let response_validator =
            jsonschema::validator_for(&descriptor.response_schema).map_err(|_| {
                CraftError::new(ErrorCode::Internal, "Invalid command response schema.")
            })?;
        if !validator.is_valid(&descriptor.example) {
            return Err(CraftError::new(
                ErrorCode::Internal,
                "Command example does not match its schema.",
            ));
        }
        if !response_validator.is_valid(&descriptor.response_example) {
            return Err(CraftError::new(
                ErrorCode::Internal,
                "Response example does not match its schema.",
            ));
        }
        if let Some(contract) = &descriptor.projection {
            let projection = contract.compile(&descriptor.response_schema, &[])?;
            let selected = projection.response_schema(&descriptor.response_schema)?;
            let validator = jsonschema::validator_for(&selected).map_err(|_| {
                CraftError::new(ErrorCode::Internal, "Invalid default projection schema.")
            })?;
            let mut example = descriptor.response_example.clone();
            example["data"] = projection.data(&example["data"])?;
            if !validator.is_valid(&example) {
                return Err(CraftError::new(
                    ErrorCode::Internal,
                    "Invalid projected response example.",
                ));
            }
        }
        self.operations.insert(
            descriptor.id.clone(),
            RegisteredOperation {
                descriptor,
                validator,
                response_validator,
                handler: None,
                typed_validator: None,
            },
        );
        Ok(())
    }

    pub fn register_typed<I: DeserializeOwned>(
        &mut self,
        mut descriptor: OperationDescriptor,
    ) -> Result<(), CraftError> {
        typed_input::<I>(&descriptor.example).map_err(|_| {
            CraftError::new(
                ErrorCode::Internal,
                "Example does not match its typed input contract.",
            )
        })?;
        descriptor
            .input_schema
            .as_object_mut()
            .ok_or_else(|| {
                CraftError::new(ErrorCode::Internal, "Typed input needs an object schema.")
            })?
            .insert(
                "x-craft-input-decoder".into(),
                Value::String("serde-strict".into()),
            );
        let command = descriptor.id.clone();
        self.register(descriptor)?;
        self.operations
            .get_mut(&command)
            .ok_or_else(|| {
                CraftError::new(ErrorCode::Internal, "Registered operation is missing.")
            })?
            .typed_validator = Some(typed_input::<I>);
        Ok(())
    }

    pub fn register_handler(
        &mut self,
        descriptor: OperationDescriptor,
        handler: Arc<dyn OperationHandler>,
    ) -> Result<(), CraftError> {
        let command = descriptor.id.clone();
        self.register(descriptor)?;
        let operation = self.operations.get_mut(&command).ok_or_else(|| {
            CraftError::new(ErrorCode::Internal, "Registered operation is missing.")
        })?;
        operation.handler = Some(handler);
        Ok(())
    }

    pub fn register_typed_handler<I: DeserializeOwned>(
        &mut self,
        descriptor: OperationDescriptor,
        handler: Arc<dyn OperationHandler>,
    ) -> Result<(), CraftError> {
        let command = descriptor.id.clone();
        self.register_typed::<I>(descriptor)?;
        self.operations
            .get_mut(&command)
            .ok_or_else(|| {
                CraftError::new(ErrorCode::Internal, "Registered operation is missing.")
            })?
            .handler = Some(handler);
        Ok(())
    }

    /// Publishes a module atomically after checking every registration and manifest binding.
    pub fn register_provider(&mut self, provider: &dyn ProviderModule) -> Result<(), CraftError> {
        let manifest = provider.manifest();
        validate_identity("/provider/id", &manifest.id)?;
        validate_identity("/provider/version", &manifest.version)?;
        if manifest.contract_version != crate::contracts::SCHEMA_VERSION
            || manifest.deployments.is_empty()
            || self.providers.contains_key(&manifest.id)
        {
            return Err(CraftError::new(
                ErrorCode::Internal,
                "Invalid or duplicate provider manifest.",
            ));
        }
        let mut deployments = std::collections::BTreeSet::new();
        for deployment in &manifest.deployments {
            validate_identity("/provider/deployment", deployment)?;
            if !deployments.insert(deployment) {
                return Err(CraftError::new(
                    ErrorCode::Internal,
                    "Duplicate provider deployment.",
                ));
            }
        }
        let mut candidate = Registry::default();
        provider.register(&mut candidate)?;
        if candidate.operations.is_empty() || !candidate.providers.is_empty() {
            return Err(CraftError::new(
                ErrorCode::Internal,
                "A provider must register its own implemented operations.",
            ));
        }
        for (id, operation) in &candidate.operations {
            if operation.descriptor.provider != manifest.id
                || !id.starts_with(&format!("{}.", manifest.id))
                || operation.handler.is_none()
                || self.operations.contains_key(id)
            {
                return Err(CraftError::new(
                    ErrorCode::Internal,
                    "Provider operation is unimplemented, mismatched or duplicate.",
                ));
            }
        }
        self.operations.extend(candidate.operations);
        self.providers.insert(manifest.id.clone(), manifest);
        Ok(())
    }

    pub fn providers(&self) -> impl Iterator<Item = &ProviderManifest> {
        self.providers.values()
    }

    pub fn prepare(&self, request: CanonicalRequest) -> Result<PreparedOperation, CraftError> {
        request.validate()?;
        self.validate(&request.command, &request.input)?;
        let operation = self.operations.get(&request.command).ok_or_else(|| {
            CraftError::new(ErrorCode::CommandUnknown, "Command is not registered.")
        })?;
        if operation.descriptor.provider != request.target.provider {
            return Err(CraftError::field(
                "/target/provider",
                "command_provider_mismatch",
            ));
        }
        if let Some(provider) = self.providers.get(&request.target.provider)
            && !provider.deployments.contains(&request.target.deployment)
        {
            return Err(CraftError::field(
                "/target/deployment",
                "provider_deployment_unsupported",
            ));
        }
        if let Some(projection) = self.projection(&request.command, &request.selection.fields)? {
            let schema = projection.response_schema(&operation.descriptor.response_schema)?;
            jsonschema::validator_for(&schema).map_err(|_| {
                CraftError::new(ErrorCode::Internal, "Invalid selected response schema.")
            })?;
        }
        let handler = operation.handler.as_ref().ok_or_else(|| {
            CraftError::new(
                ErrorCode::UnsupportedCapability,
                "Operation has no execution handler.",
            )
        })?;
        handler.validate(&request)?;
        PreparedOperation::new(
            request,
            operation.descriptor.effect,
            descriptor_digest(&operation.descriptor)?,
        )
    }

    pub fn execute(
        &self,
        prepared: &PreparedOperation,
        context: &mut dyn OperationContext,
    ) -> Result<Value, CraftError> {
        self.run_prepared(prepared, context, false)
    }

    /// Rechecks the sealed descriptor before a host allocates authenticated execution ports.
    pub fn prepared_descriptor(
        &self,
        prepared: &PreparedOperation,
    ) -> Result<&OperationDescriptor, CraftError> {
        let descriptor = self.descriptor(&prepared.request().command)?;
        if descriptor_digest(descriptor)? != prepared.descriptor_digest() {
            return Err(CraftError::new(
                ErrorCode::VersionConflict,
                "Operation contract changed after preparation.",
            ));
        }
        Ok(descriptor)
    }

    pub fn reconcile(
        &self,
        prepared: &PreparedOperation,
        context: &mut dyn OperationContext,
    ) -> Result<Value, CraftError> {
        self.run_prepared(prepared, context, true)
    }

    fn run_prepared(
        &self,
        prepared: &PreparedOperation,
        context: &mut dyn OperationContext,
        reconcile: bool,
    ) -> Result<Value, CraftError> {
        self.prepared_descriptor(prepared)?;
        let operation = self
            .operations
            .get(&prepared.request().command)
            .ok_or_else(|| {
                CraftError::new(
                    ErrorCode::CommandUnknown,
                    "Prepared operation is not registered.",
                )
            })?;
        let handler = operation.handler.as_ref().ok_or_else(|| {
            CraftError::new(
                ErrorCode::UnsupportedCapability,
                "Operation has no execution handler.",
            )
        })?;
        if reconcile
            && matches!(
                operation.descriptor.reconciliation,
                ReconciliationClass::NotApplicable
            )
        {
            return Err(CraftError::new(
                ErrorCode::UnsupportedCapability,
                "Operation declares no reconciliation strategy.",
            ));
        }
        context.verify_operation(prepared)?;
        context.checkpoint()?;
        let mut checked = CheckedContext {
            routes: &operation.descriptor.routes,
            effect: operation.descriptor.effect,
            host: context,
        };
        let data = if reconcile {
            handler.reconcile(prepared, &mut checked)
        } else {
            handler.execute(prepared, &mut checked)
        }?;
        let projection = self.projection(
            &operation.descriptor.id,
            &prepared.request().selection.fields,
        )?;
        let data = match &projection {
            Some(projection) => projection
                .data(&data)
                .map_err(|_| invalid_result(operation.descriptor.effect))?,
            None => data,
        };
        let envelope = serde_json::to_value(Envelope::success(
            &operation.descriptor.id,
            "validation".into(),
            data.clone(),
        ))
        .map_err(|_| CraftError::new(ErrorCode::Internal, "Cannot encode handler result."))?;
        let valid = match projection {
            Some(projection) => {
                let schema = projection.response_schema(&operation.descriptor.response_schema)?;
                jsonschema::validator_for(&schema)
                    .map_err(|_| {
                        CraftError::new(ErrorCode::Internal, "Invalid selected response schema.")
                    })?
                    .is_valid(&envelope)
            }
            None => operation.response_validator.is_valid(&envelope),
        };
        if !valid {
            return Err(invalid_result(operation.descriptor.effect));
        }
        Ok(data)
    }

    pub fn descriptor(&self, command: &str) -> Result<&OperationDescriptor, CraftError> {
        self.operations
            .get(command)
            .map(|operation| &operation.descriptor)
            .ok_or_else(|| {
                CraftError::new(
                    ErrorCode::CommandUnknown,
                    "Command is not implemented; use command find.",
                )
            })
    }

    pub fn descriptors(&self) -> impl Iterator<Item = &OperationDescriptor> {
        self.operations
            .values()
            .map(|operation| &operation.descriptor)
    }

    pub fn validate(&self, command: &str, input: &Value) -> Result<(), CraftError> {
        let operation = self.operations.get(command).ok_or_else(|| {
            CraftError::new(
                ErrorCode::CommandUnknown,
                "Cannot validate an unknown command.",
            )
        })?;
        let issues: Vec<_> = operation
            .validator
            .iter_errors(input)
            .take(5)
            .map(|error| {
                FieldIssue::new(
                    error.instance_path().to_string(),
                    error.schema_path().to_string(),
                )
            })
            .collect();
        if !issues.is_empty() {
            let mut error = CraftError::new(
                ErrorCode::InputInvalid,
                "Input does not match the command schema.",
            );
            error.details.issues = issues;
            error.next_action = format!("Inspect schema --command {command}.");
            return Err(error);
        }
        if let Some(validate) = operation.typed_validator {
            validate(input).map_err(|mut error| {
                error.next_action = format!("Inspect schema --command {command}.");
                error
            })?;
        }
        Ok(())
    }

    pub fn schema_digest(&self, command: &str, response: bool) -> Result<String, CraftError> {
        let descriptor = self.descriptor(command)?;
        canonical_digest(if response {
            &descriptor.response_schema
        } else {
            &descriptor.input_schema
        })
    }

    pub fn projection(
        &self,
        command: &str,
        fields: &[String],
    ) -> Result<Option<CompiledProjection>, CraftError> {
        let descriptor = self.descriptor(command)?;
        match &descriptor.projection {
            Some(contract) => contract
                .compile(&descriptor.response_schema, fields)
                .map(Some),
            None if fields.is_empty() => Ok(None),
            None => Err(CraftError::new(
                ErrorCode::FieldUnknown,
                "This command does not support field selection.",
            )),
        }
    }
}

fn typed_input<I: DeserializeOwned>(input: &Value) -> Result<(), CraftError> {
    serde_path_to_error::deserialize::<_, I>(input.clone())
        .map(|_| ())
        .map_err(|error| {
            let mut path = String::new();
            for segment in error.path() {
                match segment {
                    serde_path_to_error::Segment::Map { key } => {
                        path = crate::input::pointer_child(&path, key)
                    }
                    serde_path_to_error::Segment::Seq { index } => {
                        path = crate::input::pointer_child(&path, &index.to_string())
                    }
                    serde_path_to_error::Segment::Enum { .. }
                    | serde_path_to_error::Segment::Unknown => {}
                }
            }
            CraftError::field(path, "typed_input_mismatch")
        })
}

fn descriptor_digest(descriptor: &OperationDescriptor) -> Result<String, CraftError> {
    canonical_digest(
        &serde_json::to_value(descriptor).map_err(|_| {
            CraftError::new(ErrorCode::Internal, "Cannot encode operation descriptor.")
        })?,
    )
}

fn invalid_result(effect: EffectClass) -> CraftError {
    let read = matches!(effect, EffectClass::Read);
    let mut error = CraftError::new(
        if read {
            ErrorCode::Internal
        } else {
            ErrorCode::WriteOutcomeUnknown
        },
        "Handler result does not match its declared response schema.",
    );
    error.details.write_state = if read {
        crate::contracts::RequestState::Failed
    } else {
        crate::contracts::RequestState::SentUnknown
    };
    if !read {
        error.next_action =
            "Inspect the operation journal and reconcile before replaying the mutation.".into();
    }
    error
}
