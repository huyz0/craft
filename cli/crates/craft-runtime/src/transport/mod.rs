//! Host-owned HTTPS execution. Provider ports never receive a raw client, URL or credential.

mod downloads;
mod multipart;
mod mutations;
mod retry;
pub use downloads::{DownloadOutcome, DownloadRequest, PreparedHttpDownload};
pub use multipart::MultipartFile;
use retry::ReadRetry;
mod routing;
pub use mutations::{MutationAttempt, PreparedHttpMutation};
pub use routing::{JiraFieldSite, JiraProjectSite};
#[cfg(test)]
mod tests;

use std::fmt;
use std::time::{Duration, UNIX_EPOCH};

use craft_core::contracts::{EffectClass, GrantRef, RequestState, TenantRef};
use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::{
    HttpMethod, NativeCall, NativeResponse, NativeRoute, OperationContext, PreparedOperation,
    RetryAfter,
};
use craft_core::input::parse_json;
use craft_core::registry::Registry;
use craft_core::storage::SecretBytes;
use reqwest::header::{
    ACCEPT, ACCEPT_ENCODING, AUTHORIZATION, CONTENT_TYPE, HeaderValue, RETRY_AFTER,
};
use reqwest::{Certificate, Client, ClientBuilder, Method, Request};
use routing::{Endpoint, denied};
use tokio::runtime::Runtime;

/// Credential binding supplied by the trusted auth host, never by provider call arguments.
pub struct HostSession {
    #[cfg(feature = "synthetic-acceptance")]
    synthetic: Option<crate::synthetic_acceptance::Loopback>,
    tenant: TenantRef,
    grant: GrantRef,
    token: SecretBytes,
}

impl fmt::Debug for HostSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("HostSession([REDACTED])")
    }
}

impl HostSession {
    /// Validates local binding/encoding only; the auth layer must separately verify access.
    pub fn new(tenant: TenantRef, grant: GrantRef, token: SecretBytes) -> Result<Self, CraftError> {
        tenant.validate()?;
        grant.validate()?;
        if !bearer_is_valid(token.expose()) {
            return Err(CraftError::field(
                "/auth",
                "invalid_bearer_encoding_or_size",
            ));
        }
        Ok(Self {
            #[cfg(feature = "synthetic-acceptance")]
            synthetic: None,
            tenant,
            grant,
            token,
        })
    }

    fn header(&self) -> Result<HeaderValue, CraftError> {
        bearer_header(&self.token)
    }

    #[cfg(feature = "synthetic-acceptance")]
    pub(crate) fn with_synthetic(
        mut self,
        route: Option<crate::synthetic_acceptance::Loopback>,
    ) -> Result<Self, CraftError> {
        if route.is_some()
            && self.token.expose() != crate::synthetic_acceptance::ACCESS_TOKEN.as_bytes()
        {
            return Err(crate::synthetic_acceptance::refusal());
        }
        self.synthetic = route;
        Ok(self)
    }
}

pub(crate) fn bearer_is_valid(bytes: &[u8]) -> bool {
    let padding = bytes
        .iter()
        .position(|byte| *byte == b'=')
        .unwrap_or(bytes.len());
    !bytes.is_empty()
        && bytes.len() <= 16_384
        && padding > 0
        && bytes[..padding]
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-._~+/".contains(byte))
        && bytes[padding..].iter().all(|byte| *byte == b'=')
}

pub(crate) fn bearer_header(token: &SecretBytes) -> Result<HeaderValue, CraftError> {
    if !bearer_is_valid(token.expose()) {
        return Err(CraftError::field(
            "/auth",
            "invalid_bearer_encoding_or_size",
        ));
    }
    let mut value = zeroize::Zeroizing::new(b"Bearer ".to_vec());
    value.extend_from_slice(token.expose());
    let mut header = HeaderValue::from_bytes(&value)
        .map_err(|_| CraftError::field("/auth", "invalid_bearer_encoding_or_size"))?;
    header.set_sensitive(true);
    Ok(header)
}

pub(crate) fn product_identity_url(tenant: &TenantRef) -> Result<url::Url, CraftError> {
    let mut url = Endpoint::atlassian(tenant)?.prefix;
    let tail: &[&str] = match tenant.provider.as_str() {
        "jira" => &["rest", "api", "3", "myself"],
        "confluence" => &["wiki", "rest", "api", "user", "current"],
        _ => return Err(denied()),
    };
    url.path_segments_mut()
        .map_err(|_| denied())?
        .pop_if_empty()
        .extend(tail);
    Ok(url)
}

#[derive(Clone)]
pub enum TlsPolicy {
    Platform,
    AdditionalCaBundle(Vec<u8>),
}

pub(crate) fn client_builder(policy: TlsPolicy) -> Result<ClientBuilder, CraftError> {
    let mut builder = Client::builder()
        .https_only(true)
        .http1_only()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .no_proxy()
        .referer(false)
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .no_zstd()
        .tls_version_min(reqwest::tls::Version::TLS_1_2)
        .tls_sslkeylogfile(false)
        .connection_verbose(false)
        .connect_timeout(Duration::from_secs(10))
        .pool_max_idle_per_host(0);
    if let TlsPolicy::AdditionalCaBundle(pem) = policy {
        if pem.is_empty() || pem.len() > 1_048_576 {
            return Err(CraftError::field(
                "/tls/ca_bundle",
                "ca_bundle_size_invalid",
            ));
        }
        let roots = Certificate::from_pem_bundle(&pem)
            .map_err(|_| CraftError::field("/tls/ca_bundle", "invalid_ca_bundle"))?;
        if roots.is_empty() || roots.len() > 32 {
            return Err(CraftError::field(
                "/tls/ca_bundle",
                "ca_bundle_count_invalid",
            ));
        }
        builder = builder.tls_certs_merge(roots);
    }
    Ok(builder)
}

pub struct HttpContext {
    field_site: Option<JiraFieldSite>,
    project_site: Option<JiraProjectSite>,
    client: Client,
    runtime: Runtime,
    endpoint: Endpoint,
    routes: Vec<NativeRoute>,
    session: HostSession,
    effect: EffectClass,
    control: InvocationControl,
    operation_digest: String,
    /// Present only for sealed safe-read descriptors; mutation sends never consult it.
    retry: Option<ReadRetry>,
    /// Sends made by the most recent read invocation, for exhaustion diagnostics.
    sends: std::cell::Cell<u8>,
}

impl HttpContext {
    /// Constructs a read context from an unchanged sealed operation and matching host session.
    /// Resource mutations require a durable send/recovery integration before they can execute.
    pub fn for_operation(
        registry: &Registry,
        operation: &PreparedOperation,
        grant: &GrantRef,
        session: HostSession,
        tls: TlsPolicy,
        control: InvocationControl,
    ) -> Result<Self, CraftError> {
        control.check()?;
        if tokio::runtime::Handle::try_current().is_ok() {
            return Err(internal());
        }
        registry.prepared_descriptor(operation)?;
        if !matches!(operation.effect(), EffectClass::Read) {
            return Err(CraftError::new(
                ErrorCode::UnsupportedCapability,
                "Mutating HTTP execution requires durable send intent and recovery.",
            ));
        }
        Self::bound(registry, operation, grant, session, tls, control)
    }

    fn bound(
        registry: &Registry,
        operation: &PreparedOperation,
        grant: &GrantRef,
        session: HostSession,
        tls: TlsPolicy,
        control: InvocationControl,
    ) -> Result<Self, CraftError> {
        control.check()?;
        if tokio::runtime::Handle::try_current().is_ok() {
            return Err(internal());
        }
        let descriptor = registry.prepared_descriptor(operation)?;
        if session.tenant != operation.request().target.tenant_ref() || &session.grant != grant {
            return Err(denied());
        }
        let endpoint = Endpoint::atlassian(&session.tenant)?;
        #[cfg(feature = "synthetic-acceptance")]
        let endpoint = if let Some(route) = &session.synthetic {
            Endpoint {
                prefix: route.product_url(endpoint.prefix)?,
            }
        } else {
            endpoint
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| internal())?;
        let client = client_builder(tls)?.build().map_err(|_| internal())?;
        Ok(Self {
            field_site: None,
            project_site: None,
            client,
            runtime,
            endpoint,
            routes: descriptor.routes.clone(),
            session,
            effect: operation.effect(),
            control,
            operation_digest: operation.digest().into(),
            retry: ReadRetry::for_descriptor(descriptor),
            sends: std::cell::Cell::new(0),
        })
    }

    /// Enables only typed project-search link parsing; requests retain their original endpoint.
    pub fn with_jira_project_site(mut self, site: JiraProjectSite) -> Result<Self, CraftError> {
        self.control.check()?;
        if self.session.tenant.provider != "jira"
            || self.session.tenant.deployment != "cloud"
            || !matches!(self.effect, EffectClass::Read)
            || self.routes.len() != 1
            || !JiraProjectSite::permits(&self.routes[0])
        {
            return Err(denied());
        }
        self.project_site = Some(site);
        Ok(self)
    }

    /// Enables only exact field-search link parsing; requests retain their original endpoint.
    pub fn with_jira_field_site(mut self, site: JiraFieldSite) -> Result<Self, CraftError> {
        self.control.check()?;
        if self.session.tenant.provider != "jira"
            || self.session.tenant.deployment != "cloud"
            || !matches!(self.effect, EffectClass::Read)
            || self.routes.len() != 1
            || !JiraFieldSite::permits(&self.routes[0])
        {
            return Err(denied());
        }
        self.field_site = Some(site);
        Ok(self)
    }

    fn route(&self, call: &NativeCall) -> Result<&NativeRoute, CraftError> {
        self.routes
            .iter()
            .find(|route| route.id == call.route)
            .ok_or_else(denied)
    }

    fn request(&self, call: &NativeCall) -> Result<(Request, usize), CraftError> {
        self.control.check()?;
        let route = self.route(call)?;
        let url = self.endpoint.route_url(route, call)?;
        if matches!(route.method, HttpMethod::Get) && call.body.is_some() {
            return Err(denied());
        }
        let method = match route.method {
            HttpMethod::Get => Method::GET,
            HttpMethod::Post => Method::POST,
            HttpMethod::Put => Method::PUT,
            HttpMethod::Patch => Method::PATCH,
            HttpMethod::Delete => Method::DELETE,
        };
        let mut request = self
            .client
            .request(method, url)
            .header(ACCEPT, "application/json")
            .header(ACCEPT_ENCODING, "identity")
            .timeout(self.control.remaining()?);
        if let Some(body) = &call.body {
            request = request.json(body);
        }
        // Authentication is added only to the fully validated, host-constructed destination.
        let request = request
            .header(AUTHORIZATION, self.session.header()?)
            .build()
            .map_err(|_| internal())?;
        Ok((request, route.response_max_bytes as usize))
    }

    fn after_attempt(&self, mut error: CraftError) -> CraftError {
        let read = matches!(self.effect, EffectClass::Read);
        error.details.write_state = if read {
            RequestState::Failed
        } else {
            RequestState::SentUnknown
        };
        if !read && error.code == ErrorCode::RemoteUnavailable {
            error.code = ErrorCode::WriteOutcomeUnknown;
        }
        error.retryable = false;
        error.next_action = if !read {
            "Inspect the operation receipt and reconcile before replay.".into()
        } else if self.sends.get() > 1 {
            format!(
                "Inspect connection/provider conditions; {} bounded attempts were already sent for this safe read.",
                self.sends.get()
            )
        } else {
            "Inspect connection/provider conditions; no automatic retry was performed.".into()
        };
        error
    }

    /// Sends a safe read up to the descriptor's bounded attempt count. Only retryable statuses
    /// and connection failures before any response are repeated; the deadline is never reset.
    fn exchange_retrying(
        &self,
        request: Request,
        maximum: usize,
    ) -> Result<NativeResponse, CraftError> {
        let mut retry = 0u8;
        loop {
            // The effect is rechecked here so no context can repeat a send it labels as a write.
            let policy = self
                .retry
                .filter(|_| matches!(self.effect, EffectClass::Read));
            let attempt = match policy {
                Some(_) => request.try_clone().ok_or_else(internal)?,
                None => {
                    self.sends.set(1);
                    return self.exchange_request(request, maximum);
                }
            };
            self.sends.set(retry.saturating_add(1));
            let outcome = self.exchange_attempt(attempt, maximum);
            retry = retry.saturating_add(1);
            let advice = match &outcome {
                Ok(response) if ReadRetry::retryable_status(response.status) => {
                    response.retry_after
                }
                Err(Attempt::Connection(_)) => None,
                _ => return outcome.map_err(Attempt::into_error),
            };
            match policy.and_then(|policy| policy.wait(retry, advice, &self.control)) {
                Some(wait) => self.pause(wait)?,
                None => return outcome.map_err(Attempt::into_error),
            }
        }
    }

    fn exchange_attempt(
        &self,
        request: Request,
        maximum: usize,
    ) -> Result<NativeResponse, Attempt> {
        self.control.check().map_err(Attempt::Final)?;
        self.runtime.block_on(async {
            let exchange = exchange_attempt(&self.client, request, maximum);
            tokio::pin!(exchange);
            loop {
                tokio::select! {
                    biased;
                    result = &mut exchange => return result,
                    _ = tokio::time::sleep(Duration::from_millis(10)) => {
                        self.control.check().map_err(Attempt::Final)?;
                    }
                }
            }
        })
    }

    /// Waits between safe-read attempts while observing cancellation and the deadline.
    fn pause(&self, wait: Duration) -> Result<(), CraftError> {
        self.runtime.block_on(async {
            let end = tokio::time::Instant::now() + wait;
            loop {
                self.control.check()?;
                let now = tokio::time::Instant::now();
                if now >= end {
                    return Ok(());
                }
                tokio::time::sleep((end - now).min(Duration::from_millis(10))).await;
            }
        })
    }

    fn exchange_request(
        &self,
        request: Request,
        maximum: usize,
    ) -> Result<NativeResponse, CraftError> {
        self.exchange_request_guarded(request, maximum, &|| Ok(()))
    }
    fn exchange_request_guarded(
        &self,
        request: Request,
        maximum: usize,
        check: &dyn Fn() -> Result<(), CraftError>,
    ) -> Result<NativeResponse, CraftError> {
        check()?;
        self.runtime.block_on(async {
            let exchange = exchange(&self.client, request, maximum);
            tokio::pin!(exchange);
            loop {
                tokio::select! {
                    biased;
                    result = &mut exchange => return result,
                    _ = tokio::time::sleep(Duration::from_millis(10)) => { self.control.check()?; check()?; }
                }
            }
        })
    }
}

fn internal() -> CraftError {
    CraftError::new(
        ErrorCode::Internal,
        "Cannot initialize trusted HTTPS execution.",
    )
}
fn remote() -> CraftError {
    CraftError::new(
        ErrorCode::RemoteUnavailable,
        "HTTPS exchange failed or returned an invalid bounded response.",
    )
}

fn network_error(error: reqwest::Error) -> CraftError {
    if error.is_timeout() {
        CraftError::new(
            ErrorCode::DeadlineExceeded,
            "HTTPS exchange exceeded its deadline.",
        )
    } else {
        remote()
    }
}

pub(crate) fn retry_after(value: Option<&HeaderValue>) -> Option<RetryAfter> {
    let value = value?.to_str().ok()?;
    if value.len() > 64 {
        return None;
    }
    if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) {
        return value.parse().ok().map(RetryAfter::DelaySeconds);
    }
    let date = httpdate::parse_http_date(value)
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?;
    u64::try_from(date.as_millis())
        .ok()
        .map(RetryAfter::AtUnixMillis)
}

/// Separates failures before any response, which a safe read may repeat, from final ones.
enum Attempt {
    Connection(CraftError),
    Final(CraftError),
}

impl Attempt {
    fn into_error(self) -> CraftError {
        match self {
            Self::Connection(error) | Self::Final(error) => error,
        }
    }
}

async fn exchange(
    client: &Client,
    request: Request,
    maximum: usize,
) -> Result<NativeResponse, CraftError> {
    exchange_attempt(client, request, maximum)
        .await
        .map_err(Attempt::into_error)
}

async fn exchange_attempt(
    client: &Client,
    request: Request,
    maximum: usize,
) -> Result<NativeResponse, Attempt> {
    let allow_empty_ok = request.method() == reqwest::Method::DELETE;
    let response = client.execute(request).await.map_err(|error| {
        // Only a connection that dropped after the request was written is repeatable. Connect,
        // DNS and certificate failures are deterministic enough to report on the first attempt.
        if error.is_timeout() || error.is_connect() {
            Attempt::Final(network_error(error))
        } else {
            Attempt::Connection(network_error(error))
        }
    })?;
    read_response(response, maximum, allow_empty_ok)
        .await
        .map_err(Attempt::Final)
}

async fn read_response(
    mut response: reqwest::Response,
    maximum: usize,
    allow_empty_ok: bool,
) -> Result<NativeResponse, CraftError> {
    let status = response.status().as_u16();
    if (300..400).contains(&status) {
        return Err(denied());
    }
    let request_id = ["atl-traceid", "x-arequestid", "x-request-id"]
        .iter()
        .find_map(|name| {
            let value = response.headers().get(*name)?.to_str().ok()?;
            (!value.is_empty()
                && value.len() <= 128
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte)))
            .then(|| value.to_owned())
        });
    let retry_after = retry_after(response.headers().get(RETRY_AFTER));
    // Bad pagination metadata must not hide a confirmed mutation response.
    let link_headers = observed_link_headers(response.headers());
    if status >= 400 {
        return Ok(NativeResponse {
            status,
            body: serde_json::Value::Null,
            request_id,
            retry_after,
            link_headers,
        });
    }
    if response
        .content_length()
        .is_some_and(|size| size > maximum as u64)
    {
        return Err(remote());
    }
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(network_error)? {
        if chunk.len() > maximum.saturating_sub(body.len()) {
            return Err(remote());
        }
        body.extend_from_slice(&chunk);
    }
    let body = decode_entity(status, &content_type, &body, maximum, allow_empty_ok)?;
    Ok(NativeResponse {
        status,
        body,
        request_id,
        retry_after,
        link_headers,
    })
}

fn decode_entity(
    status: u16,
    content_type: &str,
    body: &[u8],
    maximum: usize,
    allow_empty_ok: bool,
) -> Result<serde_json::Value, CraftError> {
    // Created responses may omit a representation; adapters still qualify the native status.
    if body.is_empty() && (matches!(status, 201 | 204 | 205) || (status == 200 && allow_empty_ok)) {
        return Ok(serde_json::Value::Null);
    }
    if !matches!(status, 204 | 205)
        && !(content_type.eq_ignore_ascii_case("application/json")
            || (content_type.starts_with("application/") && content_type.ends_with("+json")))
    {
        return Err(remote());
    }
    parse_json(body, maximum).map_err(|_| remote())
}

fn observed_link_headers(headers: &reqwest::header::HeaderMap) -> Option<Vec<String>> {
    let mut values = Vec::new();
    let mut total = 0usize;
    for value in headers.get_all("link") {
        let value = value.to_str().ok()?;
        total = total.checked_add(value.len())?;
        if values.len() >= 16 || total > 8192 {
            return None;
        }
        values.push(value.to_owned());
    }
    Some(values)
}

impl OperationContext for HttpContext {
    fn verify_operation(&self, operation: &PreparedOperation) -> Result<(), CraftError> {
        if self.operation_digest != operation.digest() {
            return Err(denied());
        }
        self.control.check()
    }
    fn checkpoint(&self) -> Result<(), CraftError> {
        self.control.check()
    }

    fn invoke(&mut self, call: &NativeCall) -> Result<NativeResponse, CraftError> {
        if tokio::runtime::Handle::try_current().is_ok() {
            return Err(internal());
        }
        let (request, maximum) = self.request(call)?;
        self.control.check()?;
        let result = self.exchange_retrying(request, maximum);
        result.map_err(|error| self.after_attempt(error))
    }

    fn normalize_next(&self, call: &NativeCall, link: &str) -> Result<NativeCall, CraftError> {
        self.control.check()?;
        let route = self.route(call)?;
        match self.endpoint.normalize_next(route, call, link) {
            Ok(next) => Ok(next),
            Err(error) => match &self.project_site {
                Some(site) => site.normalize(route, call, link),
                None => match &self.field_site {
                    Some(site) => site.normalize(route, call, link),
                    None => Err(error),
                },
            },
        }
    }
}
