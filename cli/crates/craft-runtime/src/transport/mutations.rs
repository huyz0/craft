use craft_core::storage::{JournalRecord, StoreError, SubmissionState};

use super::*;
use crate::journal::{MutationAuthority, NativeJournal, SendClaim, StepOutcome};

#[cfg(test)]
mod tests;

/// One fully constructed request. Sending consumes it, and no raw client or URL escapes.
pub struct PreparedHttpMutation<'a> {
    registry: &'a Registry,
    operation: &'a PreparedOperation,
    context: HttpContext,
    call: NativeCall,
    request: Request,
    maximum: usize,
    upload: Option<std::sync::Arc<super::multipart::UploadState>>,
    #[cfg(test)]
    cancel_before_exchange: Option<craft_core::control::CancellationToken>,
}

/// Transport evidence and local persistence are independent. Neither authorizes replay.
pub struct MutationAttempt {
    pub response: Result<NativeResponse, CraftError>,
    pub pending_evidence: Result<JournalRecord, StoreError>,
    /// A bounded native acknowledgment that did not qualify complete source framing.
    pub incomplete_upload_response: Option<NativeResponse>,
    claim: SendClaim,
    command: String,
    route: String,
}

impl<'a> PreparedHttpMutation<'a> {
    /// The trusted host must establish actor access and operation policy separately.
    pub fn prepare(
        registry: &'a Registry,
        operation: &'a PreparedOperation,
        session: HostSession,
        tls: TlsPolicy,
        control: InvocationControl,
        call: &NativeCall,
    ) -> Result<Self, CraftError> {
        if matches!(operation.effect(), EffectClass::Read) {
            return Err(denied());
        }
        let grant = session.grant.clone();
        let context = HttpContext::bound(registry, operation, &grant, session, tls, control)?;
        Self::freeze(registry, operation, context, call)
    }

    fn freeze(
        registry: &'a Registry,
        operation: &'a PreparedOperation,
        context: HttpContext,
        call: &NativeCall,
    ) -> Result<Self, CraftError> {
        if super::multipart::requires_multipart(operation)
            || matches!(context.route(call)?.method, HttpMethod::Get)
        {
            return Err(denied());
        }
        let (request, maximum) = context.request(call)?;
        Ok(Self {
            registry,
            operation,
            context,
            call: call.clone(),
            request,
            maximum,
            upload: None,
            #[cfg(test)]
            cancel_before_exchange: None,
        })
    }

    /// Seals a compiled single-file upload protocol; scope/principal/permission proof remains with
    /// the host.
    pub fn prepare_jira_upload(
        registry: &'a Registry,
        operation: &'a PreparedOperation,
        session: HostSession,
        tls: TlsPolicy,
        control: InvocationControl,
        call: &NativeCall,
        file: super::MultipartFile,
    ) -> Result<Self, CraftError> {
        let grant = session.grant.clone();
        let context = HttpContext::bound(registry, operation, &grant, session, tls, control)?;
        Self::freeze_upload(registry, operation, context, call, file)
    }
    fn freeze_upload(
        registry: &'a Registry,
        operation: &'a PreparedOperation,
        context: HttpContext,
        call: &NativeCall,
        file: super::MultipartFile,
    ) -> Result<Self, CraftError> {
        let parts = super::multipart::seal(&context, operation, call, file)?;
        Ok(Self {
            registry,
            operation,
            context,
            call: parts.call,
            request: parts.request,
            maximum: parts.maximum,
            upload: Some(parts.state),
            #[cfg(test)]
            cancel_before_exchange: None,
        })
    }

    /// A failure before the claim returns no attempt. Once claimed, all exchange outcomes
    /// retain their claim/evidence and require operation-specific interpretation.
    pub fn send(
        self,
        journal: &NativeJournal,
        authority: &MutationAuthority<'_>,
        step: &str,
    ) -> Result<MutationAttempt, CraftError> {
        self.send_checked(journal, authority, step, || Ok(()))
    }

    /// Rechecks trusted host preconditions after claim acquisition and before polling HTTP.
    /// Failure keeps the claimed request uncertain and never authorizes another send.
    pub fn send_checked(
        self,
        journal: &NativeJournal,
        authority: &MutationAuthority<'_>,
        step: &str,
        preflight: impl FnOnce() -> Result<(), CraftError>,
    ) -> Result<MutationAttempt, CraftError> {
        self.send_with_checks(journal, authority, step, preflight, &|| Ok(()))
    }
    /// Rechecks a borrowed host guard before the claim and throughout long-running I/O.
    pub fn send_guarded(
        self,
        journal: &NativeJournal,
        authority: &MutationAuthority<'_>,
        step: &str,
        guard: impl Fn() -> Result<(), CraftError>,
    ) -> Result<MutationAttempt, CraftError> {
        self.send_with_checks(journal, authority, step, &guard, &guard)
    }
    /// Keeps claim-time preparation checks separate from the ongoing I/O policy guard.
    pub fn send_with_guards(
        self,
        journal: &NativeJournal,
        authority: &MutationAuthority<'_>,
        step: &str,
        before_send: impl FnOnce() -> Result<(), CraftError>,
        ongoing: impl Fn() -> Result<(), CraftError>,
    ) -> Result<MutationAttempt, CraftError> {
        self.send_with_checks(journal, authority, step, before_send, &ongoing)
    }
    fn send_with_checks(
        self,
        journal: &NativeJournal,
        authority: &MutationAuthority<'_>,
        step: &str,
        preflight: impl FnOnce() -> Result<(), CraftError>,
        guard: &dyn Fn() -> Result<(), CraftError>,
    ) -> Result<MutationAttempt, CraftError> {
        if tokio::runtime::Handle::try_current().is_ok() {
            self.context.runtime.shutdown_background();
            return Err(internal());
        }
        self.context.control.check()?;
        guard()?;
        if authority.grant != &self.context.session.grant {
            return Err(denied());
        }
        let claim = journal.for_invocation(&self.context.control).claim_send(
            self.registry,
            self.operation,
            authority,
            step,
            &self.call,
        )?;
        #[cfg(test)]
        if let Some(token) = &self.cancel_before_exchange {
            token.cancel();
        }
        // The claim may wait for durable local state. Recheck before polling the exchange.
        let mut response = self
            .context
            .control
            .check()
            .and_then(|()| preflight())
            .and_then(|()| self.context.control.check())
            .and_then(|()| {
                self.context
                    .exchange_request_guarded(self.request, self.maximum, guard)
            })
            .map_err(|error| self.context.after_attempt(error));
        let mut incomplete_upload_response = None;
        if let Some(upload) = &self.upload {
            (response, incomplete_upload_response) =
                qualify_upload_response(&self.context, upload, response);
        }
        let pending_evidence = claim.record_outcome(&StepOutcome {
            state: SubmissionState::Unknown,
            confirmed_resources: Vec::new(),
            job: None,
        });
        Ok(MutationAttempt {
            response,
            pending_evidence,
            incomplete_upload_response,
            claim,
            command: self.operation.request().command.clone(),
            route: self.call.route,
        })
    }
}

impl MutationAttempt {
    pub fn binding(&self) -> &craft_core::storage::OperationBinding {
        self.claim.binding()
    }

    pub fn step(&self) -> &str {
        self.claim.step()
    }

    pub fn command(&self) -> &str {
        &self.command
    }

    pub fn route(&self) -> &str {
        &self.route
    }

    /// Records provider-qualified evidence, including after cancellation, without sending again.
    pub fn record_outcome(&self, outcome: &StepOutcome) -> Result<JournalRecord, StoreError> {
        self.claim.record_outcome(outcome)
    }
}

fn qualify_upload_response(
    context: &HttpContext,
    upload: &super::multipart::UploadState,
    response: Result<NativeResponse, CraftError>,
) -> (Result<NativeResponse, CraftError>, Option<NativeResponse>) {
    if (response.is_err()
        || response
            .as_ref()
            .is_ok_and(|response| response.status < 300))
        && let Some(error) = upload.failure(response.is_ok())
    {
        return (Err(context.after_attempt(error)), response.ok());
    }
    (response, None)
}
