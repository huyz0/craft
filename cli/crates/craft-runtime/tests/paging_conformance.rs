//! The shared paging contract run against every native style and real list adapters.
//!
//! Real adapters read simulated native wire responses through their own `read_page`, with
//! continuation links normalized the way the host normalizes them. The same subjects then run
//! through the bounded scan driver to show that scans reuse the adapter unchanged.
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use craft_core::contracts::{AuthMode, CanonicalRequest, ResourceRef, Selection};
use craft_core::control::{CancellationToken, InvocationControl};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::{NativeCall, NativeResponse, OperationContext, PreparedOperation};
use craft_core::input::canonical_digest;
use craft_core::pagination::{
    ListBinding, ListPage, ListRequest, NativePage, NativePosition, PageSource,
};
use craft_core::profiles::ProfileInput;
use craft_core::registry::Registry;
use craft_core::scan::{ScanCaps, ScanOutcome};
use craft_runtime::profiles::ProfileStore;
use craft_runtime::scan::{self, ScanCheckpoint, ScanInputs};
use craft_test_support::paging::contract::{self, ConformanceSubject, CountingSource, Pager};
use craft_test_support::paging::{Fault, Style, SyntheticService, dataset};
use craft_test_support::state::ManualClock;
use serde_json::{Value, json};

type TestResult = Result<(), Box<dyn std::error::Error>>;
const CLOUD: &str = "00000000-1111-2222-3333-444444444444";

struct Engine {
    parent: PathBuf,
    root: PathBuf,
    store: ProfileStore,
    clock: ManualClock,
    control: InvocationControl,
    grant: craft_core::contracts::GrantRef,
}

impl Engine {
    fn new(name: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
        fs::create_dir_all(&parent)?;
        let parent = parent.canonicalize()?;
        let root = craft_test_support::state::fixture_root(
            &parent,
            name,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos(),
        );
        let clock = ManualClock::new(10_000);
        // Cursor expiry follows the manual clock; the invocation deadline follows real time.
        let control = InvocationControl::new(
            Arc::new(craft_runtime::control::SystemClock::default()),
            CancellationToken::default(),
            600_000,
        )?;
        let store = ProfileStore::open(&root, &control)?;
        let profile = store.create(
            &ProfileInput {
                name: "factory".into(),
                provider: "jira".into(),
                deployment: "cloud".into(),
                auth_mode: AuthMode::Service,
                grant: None,
                tenant: Some(CLOUD.into()),
                defaults: BTreeMap::new(),
            },
            &control,
            |_| Ok(()),
        )?;
        Ok(Self {
            parent,
            root,
            store,
            clock,
            control,
            grant: profile.grant,
        })
    }

    fn binding(&self, provider: &str, operation: &str, query: Value) -> ListBinding {
        let digest = canonical_digest(&json!({"operation":operation})).unwrap_or_default();
        ListBinding {
            operation: operation.into(),
            descriptor_digest: digest.clone(),
            schema_digest: digest.clone(),
            projection_digest: digest,
            api_revision: "conformance".into(),
            target: ResourceRef {
                provider: provider.into(),
                deployment: "cloud".into(),
                tenant: CLOUD.into(),
                resource_type: "site".into(),
                id: CLOUD.into(),
            },
            grant: self.grant.clone(),
            actor: "actor-one".into(),
            query: BTreeMap::from([("input".into(), query)]),
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        if let Ok(root) = self.root.canonicalize()
            && root.starts_with(&self.parent)
            && root != self.parent
        {
            let _ = fs::remove_dir_all(root);
        }
    }
}

impl Pager for Engine {
    fn page(
        &mut self,
        binding: &ListBinding,
        cursor: Option<&str>,
        limit: u16,
        max_bytes: usize,
        source: &mut dyn PageSource,
    ) -> Result<ListPage, CraftError> {
        self.store.provider_page(
            ListRequest {
                binding,
                cursor,
                limit,
            },
            &self.clock,
            &self.control,
            source,
            |page| Ok(serde_json::to_vec(page).map_or(usize::MAX, |b| b.len()) <= max_bytes),
        )
    }
    fn recovery_bytes(&self) -> usize {
        serde_json::to_vec(&craft_runtime::pagination::recovery_probe()).map_or(0, |b| b.len())
    }
}

/// Synthetic services in each native style, run as conformance subjects.
struct Synthetic {
    style: Style,
    binding: ListBinding,
    changed: ListBinding,
}

impl ConformanceSubject for Synthetic {
    fn name(&self) -> String {
        format!("synthetic {:?}", self.style)
    }
    fn style(&self) -> Style {
        self.style
    }
    fn source(
        &self,
        records: usize,
        page_size: usize,
        faults: BTreeMap<usize, Fault>,
    ) -> Box<dyn CountingSource + '_> {
        let mut service = SyntheticService::new(self.style, page_size, dataset(records, 4));
        service.faults = faults;
        Box::new(service)
    }
    fn binding(&self) -> ListBinding {
        self.binding.clone()
    }
    fn changed_binding(&self) -> ListBinding {
        self.changed.clone()
    }
    fn expected_ids(&self, records: usize) -> Vec<String> {
        craft_test_support::paging::ids(&dataset(records, 0))
    }
    fn id(&self, item: &Value) -> Option<String> {
        item["id"].as_str().map(String::from)
    }
    fn can_repeat_position(&self) -> bool {
        true
    }
}

/// A real provider adapter's native wire, simulated over an offset-addressed dataset.
trait Wire {
    fn name(&self) -> &'static str;
    fn style(&self) -> Style;
    fn provider(&self) -> &'static str;
    fn registry(&self) -> Result<Registry, CraftError>;
    fn request(&self, page_size: usize) -> CanonicalRequest;
    fn record(&self, id: u64) -> Value;
    fn offset(&self, call: &NativeCall) -> usize;
    /// The native body for `records`, advertising `next` when present.
    fn body(
        &self,
        call: &NativeCall,
        records: &[Value],
        offset: usize,
        page_size: usize,
        total: usize,
        next: Option<usize>,
    ) -> Value;
    fn read(
        &self,
        operation: &PreparedOperation,
        context: &mut dyn OperationContext,
        position: Option<&NativePosition>,
    ) -> Result<NativePage, CraftError>;
    fn can_repeat_position(&self) -> bool {
        true
    }
}

struct WireState {
    records: Vec<Value>,
    faults: BTreeMap<usize, Fault>,
    fetches: usize,
}

struct WireContext<'a> {
    wire: &'a dyn Wire,
    page_size: usize,
    state: &'a RefCell<WireState>,
}

impl OperationContext for WireContext<'_> {
    fn checkpoint(&self) -> Result<(), CraftError> {
        Ok(())
    }
    fn invoke(&mut self, call: &NativeCall) -> Result<NativeResponse, CraftError> {
        let mut state = self.state.borrow_mut();
        let index = state.fetches;
        state.fetches += 1;
        let offset = self.wire.offset(call);
        let mut repeat = false;
        match state.faults.get(&index).cloned() {
            Some(Fault::InsertFront(_)) => {
                let record = self.wire.record(900_001);
                state.records.insert(0, record);
            }
            Some(Fault::Delete(_)) => {
                if !state.records.is_empty() {
                    state.records.remove(0);
                }
            }
            Some(Fault::RepeatPosition) => repeat = true,
            Some(Fault::RateLimited) => {
                return Ok(NativeResponse {
                    status: 429,
                    body: json!({}),
                    request_id: None,
                    retry_after: None,
                    link_headers: None,
                });
            }
            _ => {}
        }
        let total = state.records.len();
        let end = (offset + self.page_size).min(total);
        let slice = state.records.get(offset..end).unwrap_or(&[]).to_vec();
        let next = if repeat {
            Some(offset)
        } else {
            (end < total).then_some(end)
        };
        Ok(NativeResponse {
            status: 200,
            body: self
                .wire
                .body(call, &slice, offset, self.page_size, total, next),
            request_id: None,
            retry_after: None,
            link_headers: Some(Vec::new()),
        })
    }
    /// Mirrors the host: a next link supplies only typed query values for the same route.
    fn normalize_next(&self, call: &NativeCall, link: &str) -> Result<NativeCall, CraftError> {
        let query = link.split_once('?').map_or("", |(_, query)| query);
        let mut parsed = BTreeMap::new();
        for pair in query.split('&').filter(|pair| !pair.is_empty()) {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            parsed.insert(name.to_owned(), value.replace("%2F", "/"));
        }
        Ok(NativeCall {
            route: call.route.clone(),
            path_parameters: call.path_parameters.clone(),
            query: parsed,
            repeated_query: call.repeated_query.clone(),
            body: None,
        })
    }
}

struct WireSource<'a> {
    wire: &'a dyn Wire,
    operation: PreparedOperation,
    page_size: usize,
    state: RefCell<WireState>,
}

impl PageSource for WireSource<'_> {
    fn check_access(
        &mut self,
        _: &ListBinding,
        control: &InvocationControl,
    ) -> Result<(), CraftError> {
        control.check()
    }
    fn fetch(
        &mut self,
        _: &ListBinding,
        position: Option<&NativePosition>,
        _: &InvocationControl,
    ) -> Result<NativePage, CraftError> {
        let mut context = WireContext {
            wire: self.wire,
            page_size: self.page_size,
            state: &self.state,
        };
        self.wire.read(&self.operation, &mut context, position)
    }
}

impl CountingSource for WireSource<'_> {
    fn fetches(&self) -> usize {
        self.state.borrow().fetches
    }
}

struct Adapter<W: Wire> {
    wire: W,
    binding: ListBinding,
    changed: ListBinding,
}

impl<W: Wire> ConformanceSubject for Adapter<W> {
    fn name(&self) -> String {
        self.wire.name().into()
    }
    fn style(&self) -> Style {
        self.wire.style()
    }
    fn source(
        &self,
        records: usize,
        page_size: usize,
        faults: BTreeMap<usize, Fault>,
    ) -> Box<dyn CountingSource + '_> {
        let registry = self.wire.registry().unwrap_or_default();
        let operation = registry
            .prepare(self.wire.request(page_size))
            .unwrap_or_else(|error| panic!("{}: {error:?}", self.wire.name()));
        Box::new(WireSource {
            wire: &self.wire,
            operation,
            page_size,
            state: RefCell::new(WireState {
                records: (1..=records as u64)
                    .map(|id| self.wire.record(id))
                    .collect(),
                faults,
                fetches: 0,
            }),
        })
    }
    fn binding(&self) -> ListBinding {
        self.binding.clone()
    }
    fn changed_binding(&self) -> ListBinding {
        self.changed.clone()
    }
    fn expected_ids(&self, records: usize) -> Vec<String> {
        (1..=records).map(|id| id.to_string()).collect()
    }
    fn id(&self, item: &Value) -> Option<String> {
        item["id"].as_str().map(String::from)
    }
    fn can_repeat_position(&self) -> bool {
        self.wire.can_repeat_position()
    }
}

fn site(provider: &str) -> ResourceRef {
    ResourceRef {
        provider: provider.into(),
        deployment: "cloud".into(),
        tenant: CLOUD.into(),
        resource_type: "site".into(),
        id: CLOUD.into(),
    }
}

fn request(provider: &str, command: &str, input: Value) -> CanonicalRequest {
    CanonicalRequest {
        command: command.into(),
        target: site(provider),
        input,
        selection: Selection::default(),
        preconditions: None,
    }
}

fn token(offset: usize) -> String {
    format!("token-{offset}")
}

struct IssueSearch;
impl Wire for IssueSearch {
    fn name(&self) -> &'static str {
        "jira issue search"
    }
    fn style(&self) -> Style {
        Style::Token
    }
    fn provider(&self) -> &'static str {
        "jira"
    }
    fn registry(&self) -> Result<Registry, CraftError> {
        let mut registry = Registry::default();
        registry.register_provider(&craft_provider_jira::issue_reads::JiraIssues)?;
        Ok(registry)
    }
    fn request(&self, page_size: usize) -> CanonicalRequest {
        request(
            "jira",
            craft_provider_jira::issue_reads::SEARCH,
            json!({"jql":"project = ED ORDER BY key","page_size":page_size,"reconcile_issue_ids":[]}),
        )
    }
    fn record(&self, id: u64) -> Value {
        json!({"id":id.to_string(),"key":format!("ED-{id}"),"fields":{"summary":format!("Issue {id}"),"project":{"id":"10","key":"ED"},"status":{"id":"1","name":"Open"}}})
    }
    fn offset(&self, call: &NativeCall) -> usize {
        call.body
            .as_ref()
            .and_then(|body| body["nextPageToken"].as_str())
            .and_then(|token| token.strip_prefix("token-"))
            .and_then(|offset| offset.parse().ok())
            .unwrap_or(0)
    }
    fn body(
        &self,
        _: &NativeCall,
        records: &[Value],
        _: usize,
        _: usize,
        _: usize,
        next: Option<usize>,
    ) -> Value {
        match next {
            Some(next) => json!({"issues":records,"isLast":false,"nextPageToken":token(next)}),
            None => json!({"issues":records,"isLast":true}),
        }
    }
    fn read(
        &self,
        operation: &PreparedOperation,
        context: &mut dyn OperationContext,
        position: Option<&NativePosition>,
    ) -> Result<NativePage, CraftError> {
        let plan = craft_provider_jira::issue_reads::FieldPlan::compile(
            craft_provider_jira::issue_reads::SEARCH,
            &[],
        )?;
        craft_provider_jira::issue_reads::read_search_page(operation, context, &plan, position)
    }
}

struct ProjectSearch;
impl Wire for ProjectSearch {
    fn name(&self) -> &'static str {
        "jira project search"
    }
    fn style(&self) -> Style {
        Style::Offset
    }
    fn provider(&self) -> &'static str {
        "jira"
    }
    fn registry(&self) -> Result<Registry, CraftError> {
        let mut registry = Registry::default();
        registry.register_provider(&craft_provider_jira::projects::JiraProjects)?;
        Ok(registry)
    }
    fn request(&self, page_size: usize) -> CanonicalRequest {
        request(
            "jira",
            craft_provider_jira::projects::COMMAND,
            json!({"page_size":page_size}),
        )
    }
    fn record(&self, id: u64) -> Value {
        json!({"id":id.to_string(),"key":format!("ED{id}"),"name":format!("Project {id}"),"projectTypeKey":"software"})
    }
    fn offset(&self, call: &NativeCall) -> usize {
        call.query
            .get("startAt")
            .and_then(|offset| offset.parse().ok())
            .unwrap_or(0)
    }
    fn body(
        &self,
        call: &NativeCall,
        records: &[Value],
        offset: usize,
        page_size: usize,
        total: usize,
        next: Option<usize>,
    ) -> Value {
        let mut body = json!({"startAt":offset,"maxResults":page_size,"isLast":next.is_none(),"total":total,"values":records});
        if let Some(next) = next {
            let mut query = call.query.clone();
            query.insert("startAt".into(), next.to_string());
            query.insert("maxResults".into(), page_size.to_string());
            let query: Vec<_> = query.iter().map(|(k, v)| format!("{k}={v}")).collect();
            body["nextPage"] = json!(format!(
                "https://conformance.atlassian.net/rest/api/3/project/search?{}",
                query.join("&")
            ));
        }
        body
    }
    fn read(
        &self,
        operation: &PreparedOperation,
        context: &mut dyn OperationContext,
        position: Option<&NativePosition>,
    ) -> Result<NativePage, CraftError> {
        craft_provider_jira::projects::read_page(operation, context, position)
    }
}

struct CommentList;
impl Wire for CommentList {
    fn name(&self) -> &'static str {
        "jira comment list"
    }
    fn style(&self) -> Style {
        Style::Offset
    }
    fn provider(&self) -> &'static str {
        "jira"
    }
    fn registry(&self) -> Result<Registry, CraftError> {
        let mut registry = Registry::default();
        craft_provider_jira::comments::register(&mut registry)?;
        Ok(registry)
    }
    fn request(&self, page_size: usize) -> CanonicalRequest {
        request(
            "jira",
            craft_provider_jira::comments::LIST,
            json!({"issue_id":"100","page_size":page_size}),
        )
    }
    fn record(&self, id: u64) -> Value {
        json!({"id":id.to_string(),"author":{"accountId":"human-one"},"updated":"2026-11-11T00:01:00.000+0000","jsdPublic":false,"body":{"type":"doc","version":1,"content":[]}})
    }
    fn offset(&self, call: &NativeCall) -> usize {
        call.query
            .get("startAt")
            .and_then(|offset| offset.parse().ok())
            .unwrap_or(0)
    }
    fn body(
        &self,
        _: &NativeCall,
        records: &[Value],
        offset: usize,
        page_size: usize,
        total: usize,
        _: Option<usize>,
    ) -> Value {
        json!({"startAt":offset,"maxResults":page_size,"total":total,"comments":records})
    }
    fn read(
        &self,
        operation: &PreparedOperation,
        context: &mut dyn OperationContext,
        position: Option<&NativePosition>,
    ) -> Result<NativePage, CraftError> {
        craft_provider_jira::comments::read_page(operation, context, position)
    }
    /// Offset pagination derives the next position from offset plus page size.
    fn can_repeat_position(&self) -> bool {
        false
    }
}

/// Confluence v2 lists continue through a relative `_links.next` carrying an opaque cursor.
struct ConfluenceList {
    name: &'static str,
    command: &'static str,
    path: &'static str,
    space: bool,
}
const PAGE_LIST: ConfluenceList = ConfluenceList {
    name: "confluence page list",
    command: craft_provider_confluence::PAGE_LIST,
    path: "/wiki/api/v2/pages",
    space: false,
};
const SPACE_LIST: ConfluenceList = ConfluenceList {
    name: "confluence space list",
    command: craft_provider_confluence::SPACE_LIST,
    path: "/wiki/api/v2/spaces",
    space: true,
};
impl Wire for ConfluenceList {
    fn name(&self) -> &'static str {
        self.name
    }
    fn style(&self) -> Style {
        Style::NextLink
    }
    fn provider(&self) -> &'static str {
        "confluence"
    }
    fn registry(&self) -> Result<Registry, CraftError> {
        let mut registry = Registry::default();
        registry.register_provider(&craft_provider_confluence::ConfluenceContext)?;
        Ok(registry)
    }
    fn request(&self, page_size: usize) -> CanonicalRequest {
        request("confluence", self.command, json!({"page_size":page_size}))
    }
    fn record(&self, id: u64) -> Value {
        if self.space {
            json!({"id":id.to_string(),"key":format!("S{id}"),"name":format!("Space {id}"),"type":"global","status":"current"})
        } else {
            json!({"id":id.to_string(),"spaceId":"100","status":"current","title":format!("Plan {id}"),"version":{"number":3}})
        }
    }
    fn offset(&self, call: &NativeCall) -> usize {
        call.query
            .get("cursor")
            .and_then(|cursor| cursor.strip_prefix("c"))
            .and_then(|offset| offset.parse().ok())
            .unwrap_or(0)
    }
    fn body(
        &self,
        call: &NativeCall,
        records: &[Value],
        _: usize,
        _: usize,
        _: usize,
        next: Option<usize>,
    ) -> Value {
        match next {
            Some(next) => {
                let mut query = call.query.clone();
                query.insert("cursor".into(), format!("c{next}"));
                let query: Vec<_> = query.iter().map(|(k, v)| format!("{k}={v}")).collect();
                json!({"results":records,"_links":{"next":format!("{}?{}", self.path, query.join("&"))}})
            }
            None => json!({"results":records,"_links":{}}),
        }
    }
    fn read(
        &self,
        operation: &PreparedOperation,
        context: &mut dyn OperationContext,
        position: Option<&NativePosition>,
    ) -> Result<NativePage, CraftError> {
        craft_provider_confluence::read_page(operation, context, position)
    }
}

/// Confluence CQL search continues through `_links.next` with a cursor and `next=true`, and
/// reports the provider's own `limit`.
struct CqlSearch;
impl Wire for CqlSearch {
    fn name(&self) -> &'static str {
        "confluence page search"
    }
    fn style(&self) -> Style {
        Style::NextLink
    }
    fn provider(&self) -> &'static str {
        "confluence"
    }
    fn registry(&self) -> Result<Registry, CraftError> {
        let mut registry = Registry::default();
        registry.register_provider(&craft_provider_confluence::ConfluenceContext)?;
        Ok(registry)
    }
    fn request(&self, page_size: usize) -> CanonicalRequest {
        request(
            "confluence",
            craft_provider_confluence::PAGE_SEARCH,
            json!({"cql":"type=page","page_size":page_size}),
        )
    }
    fn record(&self, id: u64) -> Value {
        json!({"content":{"id":id.to_string(),"type":"page","status":"current","title":format!("Plan {id}"),"space":{"id":100},"version":{"number":3}}})
    }
    fn offset(&self, call: &NativeCall) -> usize {
        call.query
            .get("cursor")
            .and_then(|cursor| cursor.strip_prefix("c"))
            .and_then(|offset| offset.parse().ok())
            .unwrap_or(0)
    }
    fn body(
        &self,
        call: &NativeCall,
        records: &[Value],
        offset: usize,
        page_size: usize,
        _: usize,
        next: Option<usize>,
    ) -> Value {
        let mut body = json!({"results":records,"size":records.len(),"start":offset,"limit":page_size,"_links":{}});
        if let Some(next) = next {
            let mut query = call.query.clone();
            query.insert("cursor".into(), format!("c{next}"));
            query.insert("next".into(), "true".into());
            let query: Vec<_> = query.iter().map(|(k, v)| format!("{k}={v}")).collect();
            body["_links"]["next"] = json!(format!("/wiki/rest/api/search?{}", query.join("&")));
        }
        body
    }
    fn read(
        &self,
        operation: &PreparedOperation,
        context: &mut dyn OperationContext,
        position: Option<&NativePosition>,
    ) -> Result<NativePage, CraftError> {
        craft_provider_confluence::read_page(operation, context, position)
    }
}

fn adapter<W: Wire>(engine: &Engine, wire: W) -> Adapter<W> {
    let input = wire.request(5).input;
    let mut changed = input.clone();
    changed["changed"] = json!(true);
    Adapter {
        binding: engine.binding(wire.provider(), wire.name(), input),
        changed: engine.binding(wire.provider(), wire.name(), changed),
        wire,
    }
}

/// The scan driver reuses the same subject and reaches the same complete, lossless result.
fn scan_subject(subject: &dyn ConformanceSubject, engine: &Engine) -> TestResult {
    let binding = subject.binding();
    let mut source = subject.source(13, 5, BTreeMap::new());
    let start = ScanCheckpoint::start(
        "conformance.list",
        json!({"input":{}}),
        &binding,
        ScanCaps {
            max_items: 1_000,
            max_pages: 100,
            max_stored_bytes: 1_048_576,
        },
        1,
    )?;
    let project = |items: &[Value]| Ok(items.to_vec());
    let id = |item: &Value| subject.id(item);
    let advice = || None;
    let advanced = scan::advance(
        &start,
        ScanInputs {
            source: source.as_mut(),
            binding: &binding,
            project: &project,
            item_id: &id,
            rate_advice: &advice,
            control: &engine.control,
            clock: &engine.clock,
        },
    )?;
    assert_eq!(
        advanced.checkpoint.outcome,
        ScanOutcome::Complete,
        "{}",
        subject.name()
    );
    let ids: Vec<String> = String::from_utf8(advanced.segment)?
        .lines()
        .map(|line| {
            serde_json::from_str::<Value>(line).map(|item| subject.id(&item).unwrap_or_default())
        })
        .collect::<Result<_, _>>()?;
    assert_eq!(ids, subject.expected_ids(13), "{}", subject.name());
    assert_eq!(source.fetches(), 3);
    Ok(())
}

#[test]
fn every_native_style_and_list_adapter_meets_the_shared_paging_contract() -> TestResult {
    let mut engine = Engine::new("paging-conformance")?;
    let mut subjects: Vec<Box<dyn ConformanceSubject>> = Vec::new();
    for style in craft_test_support::paging::ALL_STYLES {
        subjects.push(Box::new(Synthetic {
            style,
            binding: engine.binding(
                "jira",
                "synthetic.list",
                json!({"style":format!("{style:?}")}),
            ),
            changed: engine.binding("jira", "synthetic.list", json!({"style":"changed"})),
        }));
    }
    subjects.push(Box::new(adapter(&engine, IssueSearch)));
    subjects.push(Box::new(adapter(&engine, ProjectSearch)));
    subjects.push(Box::new(adapter(&engine, CommentList)));
    subjects.push(Box::new(adapter(&engine, PAGE_LIST)));
    subjects.push(Box::new(adapter(&engine, SPACE_LIST)));
    subjects.push(Box::new(adapter(&engine, CqlSearch)));
    let mut styles = std::collections::BTreeSet::new();
    for subject in &subjects {
        // Expire earlier subjects' cursors so each subject starts with free cursor capacity.
        engine
            .clock
            .advance(std::time::Duration::from_secs(1_000))?;
        let report = contract::run(subject.as_ref(), &mut engine)?;
        assert!(
            report.cases.len() >= 11,
            "{}: {:?}",
            subject.name(),
            report.cases
        );
        // Live insertion before a later offset page is visible as a duplicate, and deletion
        // as a gap; both are declared limitations of a live traversal, not hidden.
        // Every simulated service addresses records by position, so an insertion before a
        // later page repeats one record and a deletion skips one.
        for (case, duplicates, missing) in &report.live_limitations {
            let expected = if case == "live insertion" {
                (1, 0)
            } else {
                (0, 1)
            };
            assert_eq!(
                (*duplicates, *missing),
                expected,
                "{}: {case}",
                subject.name()
            );
        }
        scan_subject(subject.as_ref(), &engine)?;
        styles.insert(format!("{:?}", subject.style()));
    }
    assert_eq!(styles.len(), 3);
    Ok(())
}

#[test]
fn a_rate_limited_adapter_page_publishes_no_successor() -> TestResult {
    let mut engine = Engine::new("paging-rate")?;
    let subject = adapter(&engine, PAGE_LIST);
    let mut source = subject.source(11, 5, BTreeMap::from([(1, Fault::RateLimited)]));
    let binding = subject.binding();
    let first = engine.page(&binding, None, 100, 1_048_576, source.as_mut())?;
    let cursor = first.page.next_cursor.ok_or("cursor")?;
    let limited = engine.page(&binding, Some(&cursor), 100, 1_048_576, source.as_mut());
    assert_eq!(limited.err().map(|e| e.code), Some(ErrorCode::RateLimited));
    // The predecessor is not consumed: the same cursor fetches the same position again.
    let retried = engine.page(&binding, Some(&cursor), 100, 1_048_576, source.as_mut())?;
    assert_eq!(
        retried.items.first().and_then(|i| i["id"].as_str()),
        Some("6")
    );
    Ok(())
}
