use craft_core::contracts::{CanonicalRequest, EffectClass, Envelope, Selection};
use craft_core::execution::{OperationContext, OperationHandler, PreparedOperation};
use craft_core::registry::{
    OperationDescriptor, ReconciliationClass, Registry, RetryClass, response_schema,
};

use super::identity_tests::{api, grant, tenant, trust};
use super::*;

#[cfg(feature = "synthetic-acceptance")]
#[test]
fn synthetic_identity_client_cannot_redirect_an_unmarked_token_or_cross_fixture_routes()
-> Result<(), Box<dyn std::error::Error>> {
    use crate::synthetic_acceptance::{ACCESS_TOKEN, CLIENT_ID, CLIENT_SECRET, Loopback};
    let issuer = TlsServer::start(Reply::json(200, &serde_json::to_vec(&valid())?, &[]))?;
    let product = TlsServer::start(Reply::Disconnect)?;
    let normal = client(
        &issuer,
        Arc::new(ManualClock::new(1_000)),
        CancellationToken::default(),
        10_000,
    )?;
    let mut unmarked = normal.acquire(&credentials()?)?;
    let fixture = ServiceOAuthClient::synthetic_fixture(
        Loopback::new(issuer.address().port(), product.address().port())?,
        TlsPolicy::AdditionalCaBundle([issuer.root(), product.root()].concat()),
        control(CancellationToken::default(), 10_000)?,
        Arc::new(ManualClock::new(1_000)),
    )?;
    assert_eq!(
        fixture
            .verify_product(&mut unmarked, &tenant("jira"), None)
            .err()
            .ok_or("Expected provenance refusal")?
            .error
            .code,
        ErrorCode::PolicyDenied
    );
    let synthetic_issuer = TlsServer::start(Reply::json(
        200,
        &serde_json::to_vec(
            &json!({"access_token":ACCESS_TOKEN,"expires_in":3600,"token_type":"Bearer","scope":"read:jira-user"}),
        )?,
        &[],
    ))?;
    let synthetic = ServiceOAuthClient::synthetic_fixture(
        Loopback::new(synthetic_issuer.address().port(), product.address().port())?,
        TlsPolicy::AdditionalCaBundle([synthetic_issuer.root(), product.root()].concat()),
        control(CancellationToken::default(), 10_000)?,
        Arc::new(ManualClock::new(1_000)),
    )?;
    let credentials = ServiceCredentials::from_provisioning(SecretBytes::new(
        serde_json::to_vec(&json!({"client_id":CLIENT_ID,"client_secret":CLIENT_SECRET}))?,
    )?)?;
    let mut marked = synthetic.acquire(&credentials)?;
    assert_eq!(
        normal
            .verify_product(&mut marked, &tenant("jira"), None)
            .err()
            .ok_or("Expected normal-client refusal")?
            .error
            .code,
        ErrorCode::PolicyDenied
    );
    assert_eq!(
        fixture
            .verify_product(&mut marked, &tenant("jira"), None)
            .err()
            .ok_or("Expected foreign-route refusal")?
            .error
            .code,
        ErrorCode::PolicyDenied
    );
    assert_eq!(product.connections(), 0);
    assert_eq!(issuer.finish()?.len(), 1);
    assert_eq!(synthetic_issuer.finish()?.len(), 1);
    Ok(())
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
            "Fixture has no executor.",
        ))
    }
}
fn registry(alternatives: Vec<Vec<String>>) -> Result<Registry, CraftError> {
    let mut registry = Registry::default();
    registry.register_handler(
        OperationDescriptor {
            id: "jira.fixture.read".into(),
            provider: "jira".into(),
            contract_version: "1.0".into(),
            api_revision: "fixture".into(),
            description: "Access scope fixture.".into(),
            effect: EffectClass::Read,
            input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
            response_schema: response_schema::<Value>()?,
            example: json!({}),
            response_example: json!(Envelope::success(
                "jira.fixture.read",
                "request".into(),
                json!({})
            )),
            projection: None,
            scope_alternatives: alternatives,
            permissions: vec![],
            tenant_conditions: vec![],
            routes: vec![],
            retry_class: RetryClass::SafeRead,
            reconciliation: ReconciliationClass::NotApplicable,
        },
        Arc::new(Handler),
    )?;
    Ok(registry)
}
fn operation(registry: &Registry) -> Result<PreparedOperation, CraftError> {
    registry.prepare(CanonicalRequest {
        command: "jira.fixture.read".into(),
        target: craft_core::contracts::ResourceRef {
            provider: "jira".into(),
            deployment: "cloud".into(),
            tenant: tenant("jira").tenant,
            resource_type: "project".into(),
            id: "10000".into(),
        },
        input: json!({}),
        selection: Selection::default(),
        preconditions: None,
    })
}

#[test]
fn sealed_access_requires_verified_product_exact_target_scopes_contract_and_freshness()
-> Result<(), Box<dyn std::error::Error>> {
    let mut response = valid();
    response["scope"] = json!("read:jira-user read:jira-work write:jira-work");
    let issuer = TlsServer::start(Reply::json(200, &serde_json::to_vec(&response)?, &[]))?;
    let product = TlsServer::start(Reply::json(
        200,
        br#"{"accountId":"actor-one","active":true}"#,
        &[],
    ))?;
    let clock = Arc::new(ManualClock::new(1_000));
    let mut client = client(&issuer, clock.clone(), CancellationToken::default(), 10_000)?;
    trust(&mut client, &[&issuer, &product])?;
    api(&mut client, &product)?;
    assert!(
        client
            .acquire(&credentials()?)?
            .into_verified_access(tenant("jira"), grant())
            .is_err()
    );
    let mut token = client.acquire(&credentials()?)?;
    client.verify_product(&mut token, &tenant("jira"), Some("actor-one"))?;
    let access = token.into_verified_access(tenant("jira"), grant())?;
    assert_eq!(access.identity().actor(), "actor-one");
    assert_eq!(access.grant(), &grant());
    assert!(!format!("{access:?}").contains(TOKEN));
    let alternatives = vec![
        vec!["not-granted".into()],
        vec!["read:jira-work".into(), "write:jira-work".into()],
    ];
    let registry = registry(alternatives)?;
    let operation = operation(&registry)?;
    access.check_operation(&registry, &operation)?;
    access.read_context(
        &registry,
        &operation,
        TlsPolicy::Platform,
        control(CancellationToken::default(), 10_000)?,
    )?;
    let changed = super::access_tests::registry(vec![vec!["read:jira-work".into()]])?;
    assert_eq!(
        access
            .check_operation(&changed, &operation)
            .err()
            .ok_or("Expected descriptor change")?
            .code,
        ErrorCode::VersionConflict
    );
    let mut other_request = operation.request().clone();
    other_request.target.tenant = "11111111-1111-2222-3333-444444444444".into();
    let other = registry.prepare(other_request)?;
    assert_eq!(
        access
            .check_operation(&registry, &other)
            .err()
            .ok_or("Expected target refusal")?
            .code,
        ErrorCode::PolicyDenied
    );
    clock.advance(Duration::from_millis(3_540_000))?;
    assert_eq!(
        access
            .check_operation(&registry, &operation)
            .err()
            .ok_or("Expected token expiry")?
            .code,
        ErrorCode::AuthRequired
    );
    assert_eq!(product.finish()?.len(), 1);
    assert_eq!(issuer.finish()?.len(), 2);
    Ok(())
}

#[test]
fn scope_alternatives_cannot_be_satisfied_by_partial_conjunction_or_empty_declarations()
-> Result<(), Box<dyn std::error::Error>> {
    let issuer = TlsServer::start(Reply::json(200, &serde_json::to_vec(&valid())?, &[]))?;
    let product = TlsServer::start(Reply::json(
        200,
        br#"{"accountId":"actor-one","active":true}"#,
        &[],
    ))?;
    let mut client = client(
        &issuer,
        Arc::new(ManualClock::new(1_000)),
        CancellationToken::default(),
        10_000,
    )?;
    trust(&mut client, &[&issuer, &product])?;
    api(&mut client, &product)?;
    let mut token = client.acquire(&credentials()?)?;
    client.verify_product(&mut token, &tenant("jira"), Some("actor-one"))?;
    let access = token.into_verified_access(tenant("jira"), grant())?;
    for alternatives in [
        vec![],
        vec![vec![]],
        vec![vec!["read:jira-user".into(), "write:jira-work".into()]],
        vec![
            vec!["read:jira-work".into()],
            vec!["write:jira-work".into()],
        ],
    ] {
        let registry = registry(alternatives)?;
        let operation = operation(&registry)?;
        assert_eq!(
            access
                .check_operation(&registry, &operation)
                .err()
                .ok_or("Expected scope refusal")?
                .code,
            ErrorCode::PermissionDenied
        );
        assert!(
            access
                .read_context(
                    &registry,
                    &operation,
                    TlsPolicy::Platform,
                    control(CancellationToken::default(), 10_000)?
                )
                .is_err()
        );
    }
    assert_eq!(product.finish()?.len(), 1);
    assert_eq!(issuer.finish()?.len(), 1);
    Ok(())
}
