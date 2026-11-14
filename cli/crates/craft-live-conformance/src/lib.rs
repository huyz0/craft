//! Opt-in live conformance harness.
//!
//! Runs the providers' real request construction and response parsing against a
//! real Atlassian Cloud site, so mocked fixtures can be checked against what the
//! service actually returns. It deliberately bypasses the production host: there
//! is no profile, policy, journal or service-account OAuth here. Requests are
//! sent with an Atlassian account API token (HTTP Basic) to the site origin,
//! whose REST paths match the `api.atlassian.com/ex/{product}/{cloudId}` gateway
//! paths that production uses.
//!
//! Nothing in this crate runs unless a test opts in explicitly. The site comes
//! from `CRAFT_LIVE_SITE` or the gitignored `cli/.craft-live.json`; the email
//! and token come from `CRAFT_LIVE_EMAIL`/`CRAFT_LIVE_TOKEN` or the Windows
//! credential written by `tools/live-credential.ps1`. Raw captures are written
//! under `target/live-captures`, which is never tracked.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::Duration;

use craft_core::contracts::{CanonicalRequest, ResourceRef, Selection};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::{
    HttpMethod, NativeCall, NativeResponse, NativeRoute, OperationContext, PreparedOperation,
    RouteSegment,
};
use craft_core::registry::{ProviderModule, Registry};
use serde_json::{Value, json};
use url::Url;
use zeroize::Zeroizing;

pub const CREDENTIAL_TARGET: &str = "craft-live-conformance";
const MAX_RESPONSE_BYTES: usize = 4 * 1_048_576;

/// Fails unless the caller asked for live checks. Ignored tests can still be run
/// with a broad `--ignored`; this keeps that from silently touching a tenant.
pub fn require_opt_in() -> Result<(), String> {
    match std::env::var("CRAFT_LIVE").as_deref() {
        Ok("1") => Ok(()),
        _ => Err("live conformance is opt-in: set CRAFT_LIVE=1 and run with --ignored".into()),
    }
}

pub struct LiveConfig {
    pub site: Url,
    pub email: String,
    token: Zeroizing<String>,
}

fn cli_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

pub fn capture_dir() -> PathBuf {
    cli_root().join("target").join("live-captures")
}

impl LiveConfig {
    pub fn load() -> Result<Self, String> {
        let site = match std::env::var("CRAFT_LIVE_SITE") {
            Ok(site) => site,
            Err(_) => {
                let path = cli_root().join(".craft-live.json");
                let text = std::fs::read_to_string(&path).map_err(|_| {
                    format!(
                        "no CRAFT_LIVE_SITE and no {}; run tools/live-credential.ps1",
                        path.display()
                    )
                })?;
                let value: Value =
                    serde_json::from_str(&text).map_err(|_| "malformed .craft-live.json")?;
                value["site"]
                    .as_str()
                    .ok_or("missing site in .craft-live.json")?
                    .to_owned()
            }
        };
        let site = Url::parse(site.trim_end_matches('/')).map_err(|_| "invalid site URL")?;
        let host = site.host_str().unwrap_or_default();
        if site.scheme() != "https" || !host.ends_with(".atlassian.net") || site.path() != "/" {
            return Err("site must be https://<name>.atlassian.net".into());
        }
        let (email, token) = match (
            std::env::var("CRAFT_LIVE_EMAIL"),
            std::env::var("CRAFT_LIVE_TOKEN"),
        ) {
            (Ok(email), Ok(token)) => (email, Zeroizing::new(token)),
            _ => read_stored_credential()?,
        };
        Ok(Self { site, email, token })
    }
}

#[cfg(windows)]
fn read_stored_credential() -> Result<(String, Zeroizing<String>), String> {
    use keyring_core::api::CredentialStoreApi;
    let store = windows_native_keyring_store::Store::new().map_err(|e| e.to_string())?;
    let modifiers = std::collections::HashMap::from([("target", CREDENTIAL_TARGET)]);
    let entry = store
        .build("craft", "live", Some(&modifiers))
        .map_err(|e| e.to_string())?;
    let missing =
        |_| format!("no '{CREDENTIAL_TARGET}' credential; run tools/live-credential.ps1 first");
    let email = entry
        .get_attributes()
        .map_err(missing)?
        .remove("username")
        .ok_or("stored credential has no username")?;
    let secret = Zeroizing::new(entry.get_secret().map_err(missing)?);
    let token = String::from_utf8(secret.to_vec()).map_err(|_| "stored token is not UTF-8")?;
    Ok((email, Zeroizing::new(token)))
}

#[cfg(not(windows))]
fn read_stored_credential() -> Result<(String, Zeroizing<String>), String> {
    Err("set CRAFT_LIVE_EMAIL and CRAFT_LIVE_TOKEN".into())
}

/// One recorded exchange. Bodies stay in the gitignored capture directory.
#[derive(Clone)]
pub struct Exchange {
    pub method: &'static str,
    pub path: String,
    pub status: u16,
    pub body: Value,
}

pub struct Live {
    config: LiveConfig,
    runtime: tokio::runtime::Runtime,
    client: reqwest::Client,
    pub cloud_id: String,
    allow_writes: bool,
}

impl Live {
    pub fn connect() -> Result<Self, String> {
        require_opt_in()?;
        let config = LiveConfig::load()?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        // reqwest needs a reactor in scope while the client is constructed.
        let guard = runtime.enter();
        let client = reqwest::Client::builder()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .no_proxy()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| e.to_string())?;
        drop(guard);
        let mut live = Self {
            config,
            runtime,
            client,
            cloud_id: String::new(),
            allow_writes: false,
        };
        let info = live.get_raw("/_edge/tenant_info", false)?;
        live.cloud_id = info.body["cloudId"]
            .as_str()
            .ok_or("tenant_info returned no cloudId")?
            .to_owned();
        Ok(live)
    }

    /// Permits non-GET routes for explicitly written write scenarios.
    pub fn allow_writes(&mut self, allow: bool) {
        self.allow_writes = allow;
    }

    pub fn site(&self) -> &Url {
        &self.config.site
    }

    /// A plain authenticated GET, used to discover real identifiers.
    pub fn get(&self, path: &str) -> Result<Exchange, String> {
        self.get_raw(path, true)
    }

    fn get_raw(&self, path: &str, auth: bool) -> Result<Exchange, String> {
        let url = self.config.site.join(path).map_err(|e| e.to_string())?;
        self.send(HttpMethod::Get, url, None, auth, true)
            .map_err(|e| format!("GET {path}: {}", e.message))
    }

    fn send(
        &self,
        method: HttpMethod,
        url: Url,
        body: Option<&Value>,
        auth: bool,
        read_effect: bool,
    ) -> Result<Exchange, CraftError> {
        let (verb, m) = match method {
            HttpMethod::Get => ("GET", reqwest::Method::GET),
            HttpMethod::Post => ("POST", reqwest::Method::POST),
            HttpMethod::Put => ("PUT", reqwest::Method::PUT),
            HttpMethod::Patch => ("PATCH", reqwest::Method::PATCH),
            HttpMethod::Delete => ("DELETE", reqwest::Method::DELETE),
        };
        // Some reads are POSTs (Jira's JQL search). They are permitted only when the
        // descriptor itself declares a read effect.
        if verb != "GET" && !self.allow_writes && !read_effect {
            return Err(failed("write routes are disabled for this live run"));
        }
        let path = match url.query() {
            Some(q) => format!("{}?{q}", url.path()),
            None => url.path().to_owned(),
        };
        let mut request = self
            .client
            .request(m, url)
            .header("accept", "application/json");
        if auth {
            request = request.basic_auth(&self.config.email, Some(self.config.token.as_str()));
        }
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = self
            .runtime
            .block_on(async move { request.send().await })
            .map_err(|e| failed(&format!("transport: {e}")))?;
        let status = response.status().as_u16();
        let bytes = self
            .runtime
            .block_on(async move { response.bytes().await })
            .map_err(|e| failed(&format!("body: {e}")))?;
        if bytes.len() > MAX_RESPONSE_BYTES {
            return Err(failed("response too large"));
        }
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|_| {
                Value::String(String::from_utf8_lossy(&bytes[..bytes.len().min(512)]).into())
            })
        };
        Ok(Exchange {
            method: verb,
            path,
            status,
            body,
        })
    }

    /// Runs one registered command through the provider handler with a live context.
    pub fn run(
        &self,
        module: &dyn ProviderModule,
        command: &str,
        target: ResourceRef,
        input: Value,
    ) -> Outcome {
        let mut registry = Registry::default();
        if let Err(error) = module.register(&mut registry) {
            return Outcome::setup(command, error);
        }
        self.run_in(&registry, command, target, input)
    }

    /// Runs the registry's generic handler. Most read handlers refuse this path
    /// and require their production host; use [`Live::run_with`] for those.
    pub fn run_in(
        &self,
        registry: &Registry,
        command: &str,
        target: ResourceRef,
        input: Value,
    ) -> Outcome {
        self.run_with(registry, command, target, input, |prepared, context| {
            registry.execute(prepared, context)
        })
    }

    /// Prepares the request through the registry (so input validation is the
    /// real one) and then calls the provider's own read entry point, the same
    /// function the production host calls, with a live context.
    pub fn run_with<F>(
        &self,
        registry: &Registry,
        command: &str,
        target: ResourceRef,
        input: Value,
        read: F,
    ) -> Outcome
    where
        F: FnOnce(&PreparedOperation, &mut dyn OperationContext) -> Result<Value, CraftError>,
    {
        let (routes, read_effect) = match registry.descriptor(command) {
            Ok(descriptor) => (
                descriptor.routes.clone(),
                matches!(descriptor.effect, craft_core::contracts::EffectClass::Read),
            ),
            Err(error) => return Outcome::setup(command, error),
        };
        let prepared = registry.prepare(CanonicalRequest {
            command: command.into(),
            target,
            input,
            selection: Selection::default(),
            preconditions: None,
        });
        let prepared: PreparedOperation = match prepared {
            Ok(prepared) => prepared,
            Err(error) => return Outcome::setup(command, error),
        };
        let mut context = LiveContext {
            live: self,
            routes,
            read_effect,
            exchanges: Vec::new(),
        };
        let result = read(&prepared, &mut context);
        let outcome = Outcome {
            command: command.into(),
            result: result.map_err(|e| format!("{:?}: {}", e.code, e.message)),
            exchanges: context.exchanges,
        };
        outcome.capture();
        outcome
    }

    pub fn target(&self, provider: &str, resource_type: &str, id: &str) -> ResourceRef {
        ResourceRef {
            provider: provider.into(),
            deployment: "cloud".into(),
            tenant: self.cloud_id.clone(),
            resource_type: resource_type.into(),
            id: id.into(),
        }
    }

    pub fn site_target(&self, provider: &str) -> ResourceRef {
        self.target(provider, "site", &self.cloud_id.clone())
    }
}

fn failed(message: &str) -> CraftError {
    CraftError::new(ErrorCode::Internal, message)
}

struct LiveContext<'a> {
    live: &'a Live,
    routes: Vec<NativeRoute>,
    read_effect: bool,
    exchanges: Vec<Exchange>,
}

impl LiveContext<'_> {
    fn route(&self, call: &NativeCall) -> Result<&NativeRoute, CraftError> {
        self.routes
            .iter()
            .find(|route| route.id == call.route)
            .ok_or_else(|| failed("call names a route the descriptor does not declare"))
    }
}

impl OperationContext for LiveContext<'_> {
    fn checkpoint(&self) -> Result<(), CraftError> {
        Ok(())
    }

    fn invoke(&mut self, call: &NativeCall) -> Result<NativeResponse, CraftError> {
        let route = self.route(call)?.clone();
        route.validate_call(call)?;
        let mut url = self.live.config.site.clone();
        {
            let mut path = url
                .path_segments_mut()
                .map_err(|_| failed("site cannot be a base"))?;
            path.clear();
            for segment in &route.path {
                match segment {
                    RouteSegment::Literal(value) => path.push(value),
                    RouteSegment::Parameter(name) => path.push(
                        call.path_parameters
                            .get(name)
                            .ok_or_else(|| failed("missing path parameter"))?,
                    ),
                };
            }
        }
        if !call.query.is_empty() || !call.repeated_query.is_empty() {
            let mut query = url.query_pairs_mut();
            for (name, value) in &call.query {
                query.append_pair(name, value);
            }
            for (name, values) in &call.repeated_query {
                for value in values {
                    query.append_pair(name, value);
                }
            }
        }
        let exchange = self.live.send(
            route.method,
            url,
            call.body.as_ref(),
            true,
            self.read_effect,
        )?;
        let response = NativeResponse {
            status: exchange.status,
            body: exchange.body.clone(),
            request_id: None,
            retry_after: None,
            link_headers: Some(Vec::new()),
        };
        self.exchanges.push(exchange);
        Ok(response)
    }

    /// Accepts a relative or same-origin continuation for the same route and keeps
    /// only the query parameters the route declares, like the production endpoint.
    fn normalize_next(&self, call: &NativeCall, link: &str) -> Result<NativeCall, CraftError> {
        let route = self.route(call)?;
        let next = self
            .live
            .config
            .site
            .join(link)
            .map_err(|_| failed("unparseable continuation"))?;
        if next.origin() != self.live.config.site.origin() {
            return Err(failed("cross-origin continuation"));
        }
        let mut normalized = call.clone();
        normalized.query.clear();
        normalized.repeated_query.clear();
        let declared: BTreeSet<&str> = route.query_parameters.iter().map(String::as_str).collect();
        let repeated: BTreeSet<&str> = route
            .repeated_query_parameters
            .iter()
            .map(String::as_str)
            .collect();
        for (name, value) in next.query_pairs() {
            if repeated.contains(name.as_ref()) {
                normalized
                    .repeated_query
                    .entry(name.into_owned())
                    .or_default()
                    .push(value.into_owned());
            } else if declared.contains(name.as_ref()) {
                normalized
                    .query
                    .insert(name.into_owned(), value.into_owned());
            } else {
                return Err(failed("continuation adds an undeclared parameter"));
            }
        }
        Ok(normalized)
    }
}

pub struct Outcome {
    pub command: String,
    pub result: Result<Value, String>,
    pub exchanges: Vec<Exchange>,
}

impl Outcome {
    fn setup(command: &str, error: CraftError) -> Self {
        Self {
            command: command.into(),
            result: Err(format!("setup {:?}: {}", error.code, error.message)),
            exchanges: Vec::new(),
        }
    }

    pub fn passed(&self) -> bool {
        self.result.is_ok()
    }

    /// One line per exchange plus the provider result, without response values.
    pub fn summary(&self) -> String {
        let mut lines = vec![format!(
            "{} => {}",
            self.command,
            match &self.result {
                Ok(_) => "PASS".to_owned(),
                Err(error) => format!("FAIL {error}"),
            }
        )];
        for exchange in &self.exchanges {
            lines.push(format!(
                "    {} {} -> {}",
                exchange.method,
                redact_path(&exchange.path),
                exchange.status
            ));
        }
        lines.join("\n")
    }

    fn capture(&self) {
        let dir = capture_dir();
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        let record = json!({
            "command": self.command,
            "result": match &self.result { Ok(v) => v.clone(), Err(e) => json!({"error": e}) },
            "exchanges": self.exchanges.iter().map(|e| json!({
                "method": e.method, "path": e.path, "status": e.status,
                "shape": shape(&e.body), "body": e.body,
            })).collect::<Vec<_>>(),
        });
        let name = self.command.replace('.', "_");
        let _ = std::fs::write(
            dir.join(format!("{name}.json")),
            serde_json::to_vec_pretty(&record).unwrap_or_default(),
        );
    }
}

/// Keeps the route shape but hides identifiers so summaries are safe to share.
pub fn redact_path(path: &str) -> String {
    path.split('?').next().unwrap_or(path).to_owned()
}

/// A value-free outline of a JSON document: each path with its JSON type.
/// Array elements collapse to `[]` so the outline is stable across page sizes.
pub fn shape(value: &Value) -> BTreeMap<String, &'static str> {
    fn walk(value: &Value, path: String, out: &mut BTreeMap<String, &'static str>) {
        let kind = match value {
            Value::Null => "null",
            Value::Bool(_) => "bool",
            Value::Number(_) => "number",
            Value::String(_) => "string",
            Value::Array(items) => {
                for item in items {
                    walk(item, format!("{path}[]"), out);
                }
                "array"
            }
            Value::Object(map) => {
                for (key, item) in map {
                    walk(item, format!("{path}/{key}"), out);
                }
                "object"
            }
        };
        out.entry(if path.is_empty() { "/".into() } else { path })
            .and_modify(|existing| {
                if *existing != kind {
                    *existing = "mixed";
                }
            })
            .or_insert(kind);
    }
    let mut out = BTreeMap::new();
    walk(value, String::new(), &mut out);
    out
}

/// Serves recorded response bodies in order, so a provider can be re-run against
/// captured live data offline (for example after a parser fix, or to isolate the
/// record a parser rejects).
pub struct ReplayContext {
    pub bodies: std::collections::VecDeque<(u16, Value)>,
}

impl OperationContext for ReplayContext {
    fn checkpoint(&self) -> Result<(), CraftError> {
        Ok(())
    }
    fn invoke(&mut self, _call: &NativeCall) -> Result<NativeResponse, CraftError> {
        let (status, body) = self
            .bodies
            .pop_front()
            .ok_or_else(|| failed("replay ran out of recorded responses"))?;
        Ok(NativeResponse {
            status,
            body,
            request_id: None,
            retry_after: None,
            link_headers: Some(Vec::new()),
        })
    }
}

/// Prepares a request against a synthetic tenant for offline replay.
pub fn prepare_offline(
    registry: &Registry,
    command: &str,
    provider: &str,
    resource_type: &str,
    id: Option<&str>,
    input: Value,
) -> Result<PreparedOperation, CraftError> {
    const TENANT: &str = "00000000-1111-2222-3333-444444444444";
    registry.prepare(CanonicalRequest {
        command: command.into(),
        target: ResourceRef {
            provider: provider.into(),
            deployment: "cloud".into(),
            tenant: TENANT.into(),
            resource_type: resource_type.into(),
            id: id.unwrap_or(TENANT).into(),
        },
        input,
        selection: Selection::default(),
        preconditions: None,
    })
}
