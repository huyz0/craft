//! Explicit bounded scans of scannable list commands into private artifacts.
//!
//! Each scannable host keeps its own profile, policy, binding, access checks and projection;
//! this module only drives the provider-neutral scan through the host's page source.
pub mod command;

use std::cell::Cell;

use craft_core::contracts::{CredentialCacheUpdate, RequestState, TenantRef};
use craft_core::control::InvocationControl;
use craft_core::error::CraftError;
use craft_core::execution::{
    NativeCall, NativeResponse, OperationContext, PreparedOperation, RetryAfter,
};
use craft_core::pagination::{ListBinding, PageSource};
use craft_runtime::auth::{ServiceOAuthClient, VerifiedServiceAccess};
use craft_runtime::scan::{Advanced, Projector};
use craft_runtime::secrets::LockedCredentials;

/// What a host lends the scan driver for one invocation.
pub struct ScanSession<'a> {
    pub source: &'a mut dyn PageSource,
    pub binding: &'a ListBinding,
    pub project: Projector<'a>,
    pub rate_advice: &'a dyn Fn() -> Option<RetryAfter>,
}

pub type ScanRunner<'r> = &'r mut dyn FnMut(ScanSession<'_>) -> Result<Advanced, CraftError>;

/// Records provider rate advice from each native response without changing the exchange.
pub(crate) struct AdviceRecorder<'a> {
    pub inner: &'a mut dyn OperationContext,
    pub last: &'a Cell<Option<RetryAfter>>,
}

impl OperationContext for AdviceRecorder<'_> {
    fn verify_operation(&self, operation: &PreparedOperation) -> Result<(), CraftError> {
        self.inner.verify_operation(operation)
    }
    fn checkpoint(&self) -> Result<(), CraftError> {
        self.inner.checkpoint()
    }
    fn invoke(&mut self, call: &NativeCall) -> Result<NativeResponse, CraftError> {
        let response = self.inner.invoke(call)?;
        self.last.set(response.retry_after);
        Ok(response)
    }
    fn normalize_next(&self, call: &NativeCall, link: &str) -> Result<NativeCall, CraftError> {
        self.inner.normalize_next(call, link)
    }
}

/// Opens verified service access, keeping any credential-cache receipt even on failure.
pub(crate) fn open_access(
    client: &ServiceOAuthClient,
    locked: &LockedCredentials<'_>,
    tenant: &TenantRef,
    control: &InvocationControl,
) -> Result<
    (VerifiedServiceAccess, Option<CredentialCacheUpdate>),
    (CraftError, Option<CredentialCacheUpdate>),
> {
    let client = client
        .for_invocation(control.clone())
        .map_err(|error| (error, None))?;
    match client.open_service_access(locked, tenant) {
        Ok(access) => {
            let cache = access.cache_update().map(|receipt| CredentialCacheUpdate {
                grant: access.grant().clone(),
                write_state: RequestState::Confirmed,
                cleanup_pending: Some(receipt.cleanup_pending),
            });
            Ok((access, cache))
        }
        Err(failure) => {
            let cache = failure
                .credential_update
                .as_ref()
                .map(|receipt| CredentialCacheUpdate {
                    grant: locked.grant(),
                    write_state: RequestState::Confirmed,
                    cleanup_pending: Some(receipt.cleanup_pending),
                })
                .or_else(|| {
                    failure
                        .credential_write_state
                        .map(|state| CredentialCacheUpdate {
                            grant: locked.grant(),
                            write_state: state,
                            cleanup_pending: None,
                        })
                });
            let mut error = failure.error;
            error.details.write_state = RequestState::NotSent;
            Err((error, cache))
        }
    }
}
