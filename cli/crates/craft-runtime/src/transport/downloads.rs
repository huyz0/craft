//! Binary reads stream only into bounded immutable staging, never a JSON response buffer.
use super::*;
use craft_core::storage::{
    ArtifactReceipt, ArtifactSpec, ArtifactStore, ArtifactWriter, StoreError,
};
use reqwest::header::{CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_RANGE, TRANSFER_ENCODING};
use sha2::{Digest, Sha256};
#[cfg(test)]
mod tests;

const MAX_BYTES: u64 = 1_073_741_824;
pub struct DownloadRequest {
    pub call: NativeCall,
    pub artifact: ArtifactSpec,
    pub expected_digest: Option<String>,
}
pub struct DownloadOutcome {
    pub artifact: Option<ArtifactReceipt>,
    pub artifact_write_state: RequestState,
    pub http_status: Option<u16>,
    pub provider_request_id: Option<String>,
    pub retry_after: Option<RetryAfter>,
    pub response_media_type: Option<String>,
    pub received_bytes: u64,
    pub error: Option<CraftError>,
}
impl DownloadOutcome {
    fn empty() -> Self {
        Self {
            artifact: None,
            artifact_write_state: RequestState::NotSent,
            http_status: None,
            provider_request_id: None,
            retry_after: None,
            response_media_type: None,
            received_bytes: 0,
            error: None,
        }
    }
}
/// A consumed, sealed GET. Destinations, headers and credentials cannot be replaced at send.
pub struct PreparedHttpDownload<'a> {
    registry: &'a Registry,
    operation: &'a PreparedOperation,
    context: HttpContext,
    request: Request,
    spec: ArtifactSpec,
    expected_digest: Option<String>,
}
fn valid_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|v| {
        v.len() == 64
            && v.bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    })
}
fn storage(error: StoreError) -> CraftError {
    CraftError::new(
        match error {
            StoreError::Cancelled => ErrorCode::Cancelled,
            StoreError::DeadlineExceeded => ErrorCode::DeadlineExceeded,
            StoreError::TooLarge => ErrorCode::ItemTooLarge,
            StoreError::Conflict => ErrorCode::VersionConflict,
            StoreError::Invalid => ErrorCode::InputInvalid,
            StoreError::OutcomeUnknown => ErrorCode::WriteOutcomeUnknown,
            _ => ErrorCode::Internal,
        },
        "Binary artifact staging or publication failed; inspect the exact artifact identity before another transfer.",
    )
}
fn digest_text(digest: Sha256) -> String {
    use std::fmt::Write;
    let mut value = String::from("sha256:");
    for byte in digest.finalize() {
        let _ = write!(&mut value, "{byte:02x}");
    }
    value
}
impl<'a> PreparedHttpDownload<'a> {
    /// Access/scope/policy proof belongs to the verified host; this seals routing and bytes.
    pub fn prepare(
        registry: &'a Registry,
        operation: &'a PreparedOperation,
        session: HostSession,
        tls: TlsPolicy,
        control: InvocationControl,
        download: DownloadRequest,
    ) -> Result<Self, CraftError> {
        let grant = session.grant.clone();
        let context =
            HttpContext::for_operation(registry, operation, &grant, session, tls, control)?;
        Self::freeze(registry, operation, context, download)
    }
    fn freeze(
        registry: &'a Registry,
        operation: &'a PreparedOperation,
        context: HttpContext,
        download: DownloadRequest,
    ) -> Result<Self, CraftError> {
        let DownloadRequest {
            call,
            artifact: spec,
            expected_digest,
        } = download;
        registry.prepared_descriptor(operation)?;
        context.verify_operation(operation)?;
        spec.validate().map_err(storage)?;
        if spec.max_bytes > MAX_BYTES
            || expected_digest.as_ref().is_some_and(|v| !valid_digest(v))
            || spec
                .source
                .as_ref()
                .is_none_or(|s| s.tenant_ref() != operation.request().target.tenant_ref())
        {
            return Err(CraftError::field(
                "/download",
                "expected_bounded_source_bound_artifact_and_optional_sha256",
            ));
        }
        if !matches!(context.route(&call)?.method, HttpMethod::Get) || call.body.is_some() {
            return Err(denied());
        }
        let (mut request, _) = context.request(&call)?;
        request
            .headers_mut()
            .insert(ACCEPT, HeaderValue::from_static("*/*"));
        Ok(Self {
            registry,
            operation,
            context,
            request,
            spec,
            expected_digest,
        })
    }
    /// Bounded pre-body attempts and at most one finish. A storage failure never triggers another GET.
    pub fn receive(
        self,
        store: &dyn ArtifactStore,
        check: impl Fn() -> Result<(), CraftError>,
    ) -> DownloadOutcome {
        let mut outcome = DownloadOutcome::empty();
        if tokio::runtime::Handle::try_current().is_ok() {
            self.context.runtime.shutdown_background();
            outcome.error = Some(internal());
            return outcome;
        }
        let result = (|| {
            self.context.control.check()?;
            check()?;
            self.registry.prepared_descriptor(self.operation)?;
            let mut writer = store.begin(self.spec.clone()).map_err(storage)?;
            self.context.control.check()?;
            check()?;
            let digest=self.context.runtime.block_on(async {
                let transfer=stream(&self.context.client,self.request,self.context.retry,self.spec.max_bytes,writer.as_mut(),&self.context.control,&check,&mut outcome);
                tokio::pin!(transfer);
                loop {tokio::select! {
                    biased;
                    result=&mut transfer=>return result,
                    _=tokio::time::sleep(Duration::from_millis(10))=>{self.context.control.check()?;check()?;}
                }}
            })?;
            if self
                .expected_digest
                .as_ref()
                .is_some_and(|expected| expected != &digest)
            {
                return Err(CraftError::new(
                    ErrorCode::VersionConflict,
                    "Downloaded bytes differ from the expected SHA-256; no artifact was published.",
                ));
            }
            self.context.control.check()?;
            check()?;
            outcome.artifact_write_state = RequestState::SentUnknown;
            match writer.finish() {
                Ok(receipt) => {
                    outcome.artifact_write_state = RequestState::Confirmed;
                    let valid = receipt.name == self.spec.name
                        && receipt.source == self.spec.source
                        && receipt.media_type == self.spec.media_type
                        && receipt.bytes == outcome.received_bytes
                        && receipt.digest == digest;
                    outcome.artifact = Some(receipt);
                    if !valid {
                        return Err(CraftError::new(
                            ErrorCode::PartialApply,
                            "Published artifact receipt differs from the observed transfer; retain both identities and inspect locally.",
                        ));
                    }
                    Ok(())
                }
                Err(error) => {
                    outcome.artifact_write_state = if error == StoreError::OutcomeUnknown {
                        RequestState::SentUnknown
                    } else {
                        RequestState::Failed
                    };
                    Err(storage(error))
                }
            }
        })();
        outcome.error=result.err().map(|mut error| {
            error.retryable=false;
            // This exchange is read-only; local publication uncertainty has its own field.
            error.details.write_state=RequestState::NotSent;
            error.next_action="Retain the artifact identity and local publication state; a safe read is retried only before streaming begins, and no transfer was repeated after that.".into();error
        });
        outcome
    }
}
#[allow(clippy::too_many_arguments)]
async fn stream(
    client: &Client,
    request: Request,
    retry: Option<ReadRetry>,
    maximum: u64,
    writer: &mut dyn ArtifactWriter,
    control: &InvocationControl,
    check: &dyn Fn() -> Result<(), CraftError>,
    outcome: &mut DownloadOutcome,
) -> Result<String, CraftError> {
    control.check()?;
    check()?;
    let mut response = send_before_body(client, request, retry, control, check).await?;
    let status = response.status().as_u16();
    outcome.http_status = Some(status);
    outcome.provider_request_id = ["atl-traceid", "x-arequestid", "x-request-id"]
        .iter()
        .find_map(|name| {
            let v = response.headers().get(*name)?.to_str().ok()?;
            (!v.is_empty()
                && v.len() <= 128
                && v.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b)))
            .then(|| v.to_owned())
        });
    outcome.retry_after = retry_after(response.headers().get(RETRY_AFTER));
    control.check()?;
    check()?;
    if status != 200 {
        return Err(CraftError::new(
            match status {
                300..=399 => ErrorCode::PolicyDenied,
                401 => ErrorCode::AuthRequired,
                403 => ErrorCode::PermissionDenied,
                429 => ErrorCode::RateLimited,
                _ => ErrorCode::RemoteUnavailable,
            },
            "Binary read did not return a complete HTTP 200 entity; redirects and partial responses were not followed or published.",
        ));
    }
    if response.headers().contains_key(CONTENT_RANGE) {
        return Err(remote());
    }
    let encodings: Vec<_> = response
        .headers()
        .get_all(CONTENT_ENCODING)
        .iter()
        .collect();
    if encodings.len() > 1
        || encodings.first().is_some_and(|v| {
            !v.to_str()
                .is_ok_and(|s| s.trim().eq_ignore_ascii_case("identity"))
        })
    {
        return Err(remote());
    }
    let transfers: Vec<_> = response
        .headers()
        .get_all(TRANSFER_ENCODING)
        .iter()
        .collect();
    if transfers.len() > 1
        || transfers.first().is_some_and(|value| {
            !value
                .to_str()
                .is_ok_and(|coding| coding.trim().eq_ignore_ascii_case("chunked"))
        })
    {
        return Err(remote());
    }
    let lengths: Vec<_> = response.headers().get_all(CONTENT_LENGTH).iter().collect();
    if lengths.len() > 1
        || !lengths.is_empty() && response.headers().contains_key(TRANSFER_ENCODING)
    {
        return Err(remote());
    }
    let length = lengths
        .first()
        .map(|v| {
            let v = v.to_str().map_err(|_| remote())?;
            if v.is_empty() || v.len() > 20 || !v.bytes().all(|b| b.is_ascii_digit()) {
                return Err(remote());
            }
            v.parse::<u64>().map_err(|_| remote())
        })
        .transpose()?;
    if length.is_some_and(|n| n > maximum) {
        return Err(CraftError::new(
            ErrorCode::ItemTooLarge,
            "Declared binary size exceeds the explicit transfer limit.",
        ));
    }
    let media = response
        .headers()
        .get(CONTENT_TYPE)
        .map(|v| v.to_str().map_err(|_| remote()))
        .transpose()?;
    if let Some(media) = media {
        if media.is_empty()
            || media.len() > 256
            || !media.is_ascii()
            || media.chars().any(char::is_control)
        {
            return Err(remote());
        }
        let media = media.split(';').next().unwrap_or("").trim();
        let parts: Vec<_> = media.split('/').collect();
        if parts.len() != 2
            || parts.iter().any(|p| {
                p.is_empty()
                    || !p
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&c))
            })
        {
            return Err(remote());
        }
        outcome.response_media_type = Some(media.to_ascii_lowercase());
    }
    let mut digest = Sha256::new();
    loop {
        control.check()?;
        check()?;
        let Some(chunk) = response.chunk().await.map_err(network_error)? else {
            break;
        };
        control.check()?;
        check()?;
        let total = outcome
            .received_bytes
            .checked_add(chunk.len() as u64)
            .ok_or_else(remote)?;
        outcome.received_bytes = total;
        if total > maximum || length.is_some_and(|n| total > n) {
            return Err(CraftError::new(
                ErrorCode::ItemTooLarge,
                "Streamed binary size exceeds the explicit limit or declared length.",
            ));
        }
        for bytes in chunk.chunks(65_536) {
            control.check()?;
            check()?;
            writer.write_all(bytes).map_err(|error| {
                storage(
                    error
                        .get_ref()
                        .and_then(|e| e.downcast_ref::<StoreError>())
                        .copied()
                        .unwrap_or(StoreError::Unavailable),
                )
            })?;
        }
        digest.update(&chunk);
    }
    if length.is_some_and(|n| n != outcome.received_bytes) {
        return Err(remote());
    }
    control.check()?;
    check()?;
    Ok(digest_text(digest))
}

/// Repeats a safe binary read only before the artifact writer has received any byte: on a
/// retryable status or a connection dropped after the request was written. The caller's deadline and
/// local checks still bound every wait and attempt.
async fn send_before_body(
    client: &Client,
    request: Request,
    retry: Option<ReadRetry>,
    control: &InvocationControl,
    check: &dyn Fn() -> Result<(), CraftError>,
) -> Result<reqwest::Response, CraftError> {
    let Some(policy) = retry else {
        return client.execute(request).await.map_err(network_error);
    };
    let mut sent = 0u8;
    loop {
        control.check()?;
        check()?;
        let attempt = request.try_clone().ok_or_else(internal)?;
        sent = sent.saturating_add(1);
        let (advice, outcome) = match client.execute(attempt).await {
            Ok(response) if ReadRetry::retryable_status(response.status().as_u16()) => (
                retry_after(response.headers().get(RETRY_AFTER)),
                Ok(response),
            ),
            Ok(response) => return Ok(response),
            Err(error) if error.is_timeout() || error.is_connect() => {
                return Err(network_error(error));
            }
            Err(error) => (None, Err(network_error(error))),
        };
        let Some(wait) = policy.wait(sent, advice, control) else {
            return outcome;
        };
        drop(outcome);
        tokio::time::sleep(wait).await;
    }
}
