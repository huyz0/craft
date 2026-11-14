use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use craft_core::contracts::AuthMode;
use craft_core::profiles::ProfileInput;
use craft_test_support::process::{RunOptions, run};
use serde::{Deserialize, Serialize};

use super::identity_tests::tenant;
use super::*;
use crate::profiles::{ProfileStore, random_id};
use crate::secrets::tests::native::NativeCleanup;
use crate::secrets::{IncompletePolicy, NativeCredentials};

const TEST: &str = "auth::service::tests::native_tests::windows_service_cache_restarts_serializes_reacquisition_and_preserves_logout";
const RESULT_PREFIX: &str = "native-service-result=";

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Action {
    Open,
    Logout,
    Inactive,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    action: Action,
    wall: u64,
    issuer_port: u16,
    product_port: u16,
    roots: Vec<u8>,
    provider: String,
    slot: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResultData {
    updated: bool,
    version: String,
    issued: Option<u64>,
    inactive: bool,
}

struct Files {
    parent: PathBuf,
    root: PathBuf,
    removable: bool,
}

impl Files {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
        fs::create_dir_all(&parent)?;
        let parent = parent.canonicalize()?;
        let root = parent.join(random_id("native-service-")?);
        Ok(Self {
            parent,
            root,
            removable: false,
        })
    }
}

impl Drop for Files {
    fn drop(&mut self) {
        if !self.removable {
            eprintln!(
                "Owned native service fixture retained at {}",
                self.root.display()
            );
            return;
        }
        if let Ok(root) = self.root.canonicalize()
            && root.starts_with(&self.parent)
            && root != self.parent
        {
            let _ = fs::remove_dir_all(root);
        }
    }
}

fn wait_for(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let started = Instant::now();
    while !path.try_exists()? {
        if started.elapsed() >= Duration::from_secs(10) {
            return Err("Native service process barrier exceeded its deadline".into());
        }
        thread::sleep(Duration::from_millis(2));
    }
    Ok(())
}

fn configured_client(request: &Request) -> Result<ServiceOAuthClient, CraftError> {
    let mut client = ServiceOAuthClient::new(
        TlsPolicy::AdditionalCaBundle(request.roots.clone()),
        control(CancellationToken::default(), 20_000)?,
        Arc::new(ManualClock::new(request.wall)),
    )?;
    client.endpoint = reqwest::Url::parse(&format!(
        "https://localhost:{}/oauth/token",
        request.issuer_port
    ))
    .map_err(|_| invalid_client())?;
    client.api_origin = Some(
        reqwest::Url::parse(&format!("https://localhost:{}", request.product_port))
            .map_err(|_| invalid_client())?,
    );
    Ok(client)
}

fn child(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let mut input = Vec::new();
    std::io::stdin().take(65_537).read_to_end(&mut input)?;
    if input.len() > 65_536 {
        return Err("Native service fixture input exceeds its bound".into());
    }
    let request: Request = serde_json::from_slice(&input)?;
    if !matches!(
        request.slot.as_str(),
        "fresh" | "jira" | "confluence" | "logout" | "waiting" | "inactive"
    ) {
        return Err("Unknown native service fixture slot".into());
    }
    let client = configured_client(&request)?;
    let profiles = ProfileStore::open(root, &control(CancellationToken::default(), 20_000)?)?;
    let profile = profiles.get(
        "native-service",
        &control(CancellationToken::default(), 20_000)?,
    )?;
    let credentials = NativeCredentials::open(&profiles)?;
    fs::write(root.join(format!("{}.ready", request.slot)), b"ready")?;
    let locked = credentials.lock(
        &profile.grant,
        &control(CancellationToken::default(), 20_000)?,
    )?;
    let result = match request.action {
        Action::Open => {
            let session = client.open_service_session(&locked, &tenant(&request.provider))?;
            assert_eq!(session.identity.actor(), "actor-one");
            assert!(!format!("{session:?}").contains(TOKEN));
            ResultData {
                updated: session.cache_update.is_some(),
                version: format!("{:?}", locked.read()?.version),
                issued: Some(session.token.issued_at_unix_ms),
                inactive: false,
            }
        }
        Action::Logout => {
            fs::write(root.join("logout.held"), b"held")?;
            wait_for(&root.join("logout.release"))?;
            let receipt = locked.logout(&locked.read()?.version)?;
            assert!(!receipt.cleanup_pending);
            assert!(locked.read()?.secret.is_none());
            ResultData {
                updated: false,
                version: format!("{:?}", receipt.version),
                issued: None,
                inactive: true,
            }
        }
        Action::Inactive => {
            let failure = client
                .open_service_session(&locked, &tenant(&request.provider))
                .err()
                .ok_or("Logged-out native service grant unexpectedly opened")?;
            no_leaks(&failure)?;
            assert_eq!(failure.error.code, ErrorCode::AuthRequired);
            assert_eq!(failure.credential_write_state, None);
            assert!(locked.read()?.secret.is_none());
            ResultData {
                updated: false,
                version: format!("{:?}", locked.read()?.version),
                issued: None,
                inactive: true,
            }
        }
    };
    drop(locked);
    println!("{RESULT_PREFIX}{}", serde_json::to_string(&result)?);
    Ok(())
}

fn execute(root: &Path, request: Request) -> Result<ResultData, String> {
    let input = serde_json::to_vec(&request).map_err(|_| "Fixture encoding failed")?;
    let binary = std::env::current_exe().map_err(|_| "Missing native test executable")?;
    let output = run(
        &binary,
        &["--ignored", "--exact", TEST, "--nocapture"],
        Some(&input),
        RunOptions {
            timeout: Duration::from_secs(30),
            max_stream_bytes: 16_384,
            environment: vec![(
                OsString::from("CRAFT_NATIVE_SERVICE_ROOT"),
                Some(root.into()),
            )],
            ..RunOptions::default()
        },
    )
    .map_err(|_| "Native service child execution failed")?;
    for bytes in [&output.stdout, &output.stderr] {
        for sentinel in [CLIENT_ID, CLIENT_SECRET, TOKEN] {
            if bytes
                .windows(sentinel.len())
                .any(|part| part == sentinel.as_bytes())
            {
                return Err("Native service child output exposed a fixture credential".into());
            }
        }
    }
    if !output.status.success() || !output.stderr.is_empty() {
        return Err(format!(
            "Native service child failed: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let output = std::str::from_utf8(&output.stdout).map_err(|_| "Invalid child output")?;
    let results: Vec<_> = output
        .lines()
        .filter_map(|line| line.strip_prefix(RESULT_PREFIX))
        .collect();
    if results.len() != 1 {
        return Err("Expected one native service result".into());
    }
    serde_json::from_str(results[0]).map_err(|_| "Invalid native service result".into())
}

struct Runner(Option<JoinHandle<Result<ResultData, String>>>);

impl Drop for Runner {
    fn drop(&mut self) {
        if let Some(handle) = self.0.take() {
            let _ = handle.join();
        }
    }
}

fn launch(root: &Path, request: Request) -> Runner {
    let root = root.to_owned();
    Runner(Some(thread::spawn(move || execute(&root, request))))
}

fn joined(mut runner: Runner) -> Result<ResultData, Box<dyn std::error::Error>> {
    let handle = runner
        .0
        .take()
        .ok_or("Native service runner already joined")?;
    Ok(handle
        .join()
        .map_err(|_| "Native service runner panicked")??)
}

fn scan_files(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        if path.is_dir() {
            scan_files(&path)?;
        } else {
            let bytes = fs::read(path)?;
            for sentinel in [CLIENT_ID, CLIENT_SECRET, TOKEN] {
                assert!(
                    !bytes
                        .windows(sentinel.len())
                        .any(|part| part == sentinel.as_bytes())
                );
            }
        }
    }
    Ok(())
}

#[test]
#[ignore = "Writes owned synthetic service credentials to the real Windows vault; run explicitly with native loopback access"]
fn windows_service_cache_restarts_serializes_reacquisition_and_preserves_logout()
-> Result<(), Box<dyn std::error::Error>> {
    if let Some(root) = std::env::var_os("CRAFT_NATIVE_SERVICE_ROOT") {
        return child(Path::new(&root));
    }
    let mut files = Files::new()?;
    let profiles =
        ProfileStore::open(&files.root, &control(CancellationToken::default(), 20_000)?)?;
    let profile = profiles.create(
        &ProfileInput {
            name: "native-service".into(),
            provider: "jira".into(),
            deployment: "cloud".into(),
            auth_mode: AuthMode::Service,
            grant: None,
            tenant: None,
            defaults: Default::default(),
        },
        &control(CancellationToken::default(), 20_000)?,
        |_| Ok(()),
    )?;
    let mut cleanup = NativeCleanup::new(&profiles, profile.grant)?;
    let issuer = TlsServer::start(Reply::json(200, &serde_json::to_vec(&valid())?, &[]))?;
    let product = TlsServer::start(Reply::json(
        200,
        br#"{"accountId":"actor-one","active":true,"type":"known"}"#,
        &[],
    ))?;
    let request = Request {
        action: Action::Open,
        wall: 1_000,
        issuer_port: issuer.address().port(),
        product_port: product.address().port(),
        roots: [issuer.root(), product.root()].concat(),
        provider: "jira".into(),
        slot: "fresh".into(),
    };
    let client = configured_client(&request)?;
    let secret = CLIENT_SECRET.repeat(400);
    let provisioned = ServiceCredentials::from_provisioning(SecretBytes::new(
        serde_json::to_vec(&json!({"client_id":CLIENT_ID,"client_secret":secret}))?,
    )?)?;
    let locked = cleanup.credentials.lock(
        &cleanup.grant,
        &control(CancellationToken::default(), 20_000)?,
    )?;
    let receipt = client.provision_grant(
        &locked,
        provisioned,
        &[tenant("jira"), tenant("confluence")],
        Some("actor-one"),
    )?;
    assert!(!receipt.generation.cleanup_pending);
    assert_eq!(receipt.products.len(), 2);
    let original = format!("{:?}", receipt.generation.version);
    let before = locked
        .read()?
        .secret
        .ok_or("Missing native service bundle")?;
    assert!(before.expose().len() > crate::vault::WINDOWS_ITEM_LIMIT);
    drop(locked);
    let mut fresh = request.clone();
    fresh.wall = 2_000;
    let reopened = execute(&files.root, fresh)?;
    assert!(!reopened.updated && !reopened.inactive);
    assert_eq!(reopened.version, original);
    assert_eq!(reopened.issued, Some(1_000));
    assert_eq!(issuer.request_count(), 1);

    // Both fresh processes announce readiness while the parent owns the grant.
    let locked = cleanup.credentials.lock(
        &cleanup.grant,
        &control(CancellationToken::default(), 20_000)?,
    )?;
    let mut renewal = request.clone();
    renewal.wall = 3_602_000;
    renewal.slot = "jira".into();
    let jira = launch(&files.root, renewal.clone());
    renewal.provider = "confluence".into();
    renewal.slot = "confluence".into();
    let confluence = launch(&files.root, renewal.clone());
    wait_for(&files.root.join("jira.ready"))?;
    wait_for(&files.root.join("confluence.ready"))?;
    assert_eq!(issuer.request_count(), 1);
    drop(locked);
    let jira = joined(jira)?;
    let confluence = joined(confluence)?;
    assert_eq!(
        usize::from(jira.updated) + usize::from(confluence.updated),
        1
    );
    assert_eq!(jira.version, confluence.version);
    assert_ne!(jira.version, original);
    assert_eq!(jira.issued, Some(3_602_000));
    assert_eq!(confluence.issued, Some(3_602_000));
    assert_eq!(issuer.request_count(), 2);
    assert_eq!(product.request_count(), 6);
    let locked = cleanup.credentials.lock(
        &cleanup.grant,
        &control(CancellationToken::default(), 20_000)?,
    )?;
    let renewed = locked.read()?;
    let bytes = renewed
        .secret
        .as_ref()
        .ok_or("Missing renewed native bundle")?;
    let stored: Value = serde_json::from_slice(bytes.expose())?;
    assert!(stored["credentials"]["client_id"] == CLIENT_ID);
    assert!(stored["credentials"]["client_secret"] == secret);
    assert!(stored["token"]["access_token"] == TOKEN);
    assert_eq!(
        stored["targets"]
            .as_array()
            .ok_or("Missing configured targets")?
            .len(),
        2
    );
    drop(locked);

    let mut logout = renewal.clone();
    logout.action = Action::Logout;
    logout.slot = "logout".into();
    let holder = launch(&files.root, logout);
    wait_for(&files.root.join("logout.held"))?;
    let mut inactive = renewal;
    inactive.action = Action::Inactive;
    inactive.wall = 7_202_000;
    inactive.slot = "waiting".into();
    let waiting = launch(&files.root, inactive.clone());
    wait_for(&files.root.join("waiting.ready"))?;
    fs::write(files.root.join("logout.release"), b"release")?;
    let logout = joined(holder)?;
    let waiting = joined(waiting)?;
    assert!(logout.inactive && waiting.inactive);
    assert_eq!(logout.version, waiting.version);
    inactive.slot = "inactive".into();
    assert_eq!(execute(&files.root, inactive)?.version, logout.version);
    let locked = cleanup.credentials.lock(
        &cleanup.grant,
        &control(CancellationToken::default(), 20_000)?,
    )?;
    let stale = locked
        .replace(&renewed.version, bytes, IncompletePolicy::KeepPrevious)
        .err()
        .ok_or("Native service stale writer resurrected logout")?;
    assert_eq!(stale.code, ErrorCode::VersionConflict);
    assert!(locked.read()?.secret.is_none());
    drop(locked);
    assert_eq!(issuer.request_count(), 2);
    assert_eq!(product.finish()?.len(), 6);
    let exchanges = issuer.finish()?;
    assert_eq!(exchanges.len(), 2);
    for exchange in exchanges {
        let form: BTreeMap<_, _> = url::form_urlencoded::parse(&exchange.body)
            .into_owned()
            .collect();
        assert!(
            form.get("client_id")
                .is_some_and(|value| value == CLIENT_ID)
        );
        assert!(
            form.get("client_secret")
                .is_some_and(|value| value == &secret)
        );
    }
    cleanup.cleanup()?;
    files.removable = true;
    drop(cleanup);
    drop(profiles);
    scan_files(&files.root)?;
    let root = files.root.clone();
    drop(files);
    assert!(!root.exists());
    Ok(())
}
