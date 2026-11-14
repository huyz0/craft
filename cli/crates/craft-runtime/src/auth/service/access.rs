use super::*;
use crate::secrets::GenerationReceipt;
use crate::transport::{HttpContext, PreparedHttpMutation};
use craft_core::execution::{NativeCall, PreparedOperation};
use craft_core::registry::Registry;

/// Sealed verified token and actor evidence retained for a composed host invocation.
pub struct VerifiedServiceAccess {
    token: ServiceToken,
    tenant: TenantRef,
    grant: GrantRef,
    identity: ProductIdentity,
    pub(super) cache_update: Option<GenerationReceipt>,
}

impl fmt::Debug for VerifiedServiceAccess {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("VerifiedServiceAccess([REDACTED])")
    }
}

impl ServiceToken {
    /// The cache constructor supplies its actual locked grant; direct hosts own that binding.
    pub fn into_verified_access(
        self,
        tenant: TenantRef,
        grant: GrantRef,
    ) -> Result<VerifiedServiceAccess, CraftError> {
        tenant.validate()?;
        grant.validate()?;
        if self.needs_reacquisition() {
            return Err(CraftError::new(
                ErrorCode::AuthRequired,
                "Verified service access requires a fresh token.",
            ));
        }
        let identity = self
            .verified
            .iter()
            .find(|identity| identity.tenant() == &tenant)
            .cloned()
            .ok_or_else(|| {
                CraftError::new(
                    ErrorCode::AuthRequired,
                    "Verified service access requires exact product identity evidence.",
                )
            })?;
        if self.actor.as_deref() != Some(identity.actor()) {
            return Err(CraftError::new(
                ErrorCode::AuthRequired,
                "Verified service actor evidence is inconsistent.",
            ));
        }
        Ok(VerifiedServiceAccess {
            token: self,
            tenant,
            grant,
            identity,
            cache_update: None,
        })
    }
}

impl ServiceOAuthClient {
    /// The same clock used for cached-token freshness and pre-acquisition cursor expiry checks.
    pub fn clock(&self) -> Arc<dyn Clock> {
        self.clock.clone()
    }

    /// Keeps issuer, TLS, fixture provenance and freshness origin while bounding a new invocation.
    pub fn for_invocation(&self, control: InvocationControl) -> Result<Self, CraftError> {
        self.control.check()?;
        control.check()?;
        if tokio::runtime::Handle::try_current().is_ok() {
            return Err(invalid_client());
        }
        Ok(Self {
            #[cfg(feature = "synthetic-acceptance")]
            synthetic: self.synthetic.clone(),
            client: self.client.clone(),
            runtime: tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| invalid_client())?,
            endpoint: self.endpoint.clone(),
            control,
            clock: self.clock.clone(),
            cache_origin_unix_ms: self.cache_origin_unix_ms,
            cache_origin_monotonic_ms: self.cache_origin_monotonic_ms,
            #[cfg(test)]
            api_origin: self.api_origin.clone(),
        })
    }
}

impl VerifiedServiceAccess {
    pub fn tenant(&self) -> &TenantRef {
        &self.tenant
    }
    pub fn grant(&self) -> &GrantRef {
        &self.grant
    }
    pub fn identity(&self) -> &ProductIdentity {
        &self.identity
    }
    pub fn cache_update(&self) -> Option<&GenerationReceipt> {
        self.cache_update.as_ref()
    }
    pub fn clock(&self) -> Arc<dyn Clock> {
        self.token.clock.clone()
    }

    pub fn check_operation(
        &self,
        registry: &Registry,
        operation: &PreparedOperation,
    ) -> Result<(), CraftError> {
        let descriptor = registry.prepared_descriptor(operation)?;
        if operation.request().target.tenant_ref() != self.tenant {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Operation target differs from the verified service product.",
            ));
        }
        if self.token.needs_reacquisition() {
            return Err(CraftError::new(
                ErrorCode::AuthRequired,
                "Service token requires reacquisition before this operation.",
            ));
        }
        let scopes = self.token.metadata.scopes.as_ref().ok_or_else(|| {
            CraftError::new(
                ErrorCode::PermissionDenied,
                "Reported token scopes are required for operation access.",
            )
        })?;
        if descriptor.scope_alternatives.is_empty()
            || !descriptor.scope_alternatives.iter().any(|alternative| {
                !alternative.is_empty()
                    && alternative.iter().all(|required| scopes.contains(required))
            })
        {
            return Err(CraftError::new(
                ErrorCode::PermissionDenied,
                "Reported service scopes do not satisfy this operation's declared alternative.",
            ));
        }
        Ok(())
    }

    fn session(&self) -> Result<HostSession, CraftError> {
        let token = SecretBytes::new(self.token.access_token.expose().to_vec())
            .map_err(|_| invalid_client())?;
        let session = HostSession::new(self.tenant.clone(), self.grant.clone(), token)?;
        #[cfg(feature = "synthetic-acceptance")]
        let session = session.with_synthetic(self.token.synthetic.clone())?;
        Ok(session)
    }

    pub fn read_context(
        &self,
        registry: &Registry,
        operation: &PreparedOperation,
        tls: TlsPolicy,
        control: InvocationControl,
    ) -> Result<HttpContext, CraftError> {
        self.check_operation(registry, operation)?;
        HttpContext::for_operation(
            registry,
            operation,
            &self.grant,
            self.session()?,
            tls,
            control,
        )
    }

    pub fn prepare_mutation<'a>(
        &self,
        registry: &'a Registry,
        operation: &'a PreparedOperation,
        tls: TlsPolicy,
        control: InvocationControl,
        call: &NativeCall,
    ) -> Result<PreparedHttpMutation<'a>, CraftError> {
        self.check_operation(registry, operation)?;
        PreparedHttpMutation::prepare(registry, operation, self.session()?, tls, control, call)
    }
    pub fn prepare_jira_upload<'a>(
        &self,
        registry: &'a Registry,
        operation: &'a PreparedOperation,
        tls: TlsPolicy,
        control: InvocationControl,
        call: &NativeCall,
        file: crate::transport::MultipartFile,
    ) -> Result<PreparedHttpMutation<'a>, CraftError> {
        self.check_operation(registry, operation)?;
        PreparedHttpMutation::prepare_jira_upload(
            registry,
            operation,
            self.session()?,
            tls,
            control,
            call,
            file,
        )
    }
    pub fn prepare_download<'a>(
        &self,
        registry: &'a Registry,
        operation: &'a PreparedOperation,
        tls: TlsPolicy,
        control: InvocationControl,
        download: crate::transport::DownloadRequest,
    ) -> Result<crate::transport::PreparedHttpDownload<'a>, CraftError> {
        self.check_operation(registry, operation)?;
        crate::transport::PreparedHttpDownload::prepare(
            registry,
            operation,
            self.session()?,
            tls,
            control,
            download,
        )
    }
}
