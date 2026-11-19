use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use craft_core::contracts::{AuthMode, Envelope, ResourceRef};
use craft_core::control::{CancellationToken, InvocationControl};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::input::{canonical_digest, content_digest};
use craft_core::pagination::{
    ListBinding, ListPage, ListRequest, NativePage, NativePosition, PageSource,
};
use craft_core::profiles::ProfileInput;
use craft_runtime::profiles::ProfileStore;
use craft_test_support::state::ManualClock;
use serde_json::{Value, json};

type TestResult = Result<(), Box<dyn std::error::Error>>;

struct Fixture {
    parent: PathBuf,
    root: PathBuf,
    clock: ManualClock,
    control: InvocationControl,
    cancellation: CancellationToken,
    binding: ListBinding,
}
impl Fixture {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
        fs::create_dir_all(&parent)?;
        let parent = parent.canonicalize()?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let root = parent.join(format!("provider-pages-{}-{stamp}", std::process::id()));
        let clock = ManualClock::new(10_000);
        let cancellation = CancellationToken::default();
        let control =
            InvocationControl::new(Arc::new(clock.clone()), cancellation.clone(), 60_000)?;
        let store = ProfileStore::open(&root, &control)?;
        let profile = store.create(
            &ProfileInput {
                name: "factory".into(),
                provider: "jira".into(),
                deployment: "cloud".into(),
                auth_mode: AuthMode::Service,
                grant: None,
                tenant: Some("00000000-1111-2222-3333-444444444444".into()),
                defaults: BTreeMap::new(),
            },
            &control,
            |_| Ok(()),
        )?;
        let digest = canonical_digest(&json!({"revision":1}))?;
        let binding = ListBinding {
            operation: "jira.project.list".into(),
            descriptor_digest: digest.clone(),
            schema_digest: digest.clone(),
            projection_digest: digest,
            api_revision: "v3".into(),
            target: ResourceRef {
                provider: "jira".into(),
                deployment: "cloud".into(),
                tenant: "00000000-1111-2222-3333-444444444444".into(),
                resource_type: "site".into(),
                id: "tenant-one".into(),
            },
            grant: profile.grant,
            actor: "actor-one".into(),
            query: BTreeMap::from([("order_by".into(), json!("key"))]),
        };
        Ok(Self {
            parent,
            root,
            clock,
            control,
            cancellation,
            binding,
        })
    }
    fn store(&self) -> Result<ProfileStore, CraftError> {
        ProfileStore::open(&self.root, &self.control)
    }
    fn raw(&self) -> Result<rusqlite::Connection, rusqlite::Error> {
        let connection = rusqlite::Connection::open(self.root.join("state.sqlite3"))?;
        connection.execute_batch("PRAGMA journal_mode=PERSIST;")?;
        Ok(connection)
    }
    fn request<'a>(&'a self, cursor: Option<&'a str>, limit: u16) -> ListRequest<'a> {
        ListRequest {
            binding: &self.binding,
            cursor,
            limit,
        }
    }
    fn list(
        &self,
        cursor: Option<&str>,
        limit: u16,
        source: &mut Source,
    ) -> Result<ListPage, CraftError> {
        self.store()?.provider_page(
            self.request(cursor, limit),
            &self.clock,
            &self.control,
            source,
            |_| Ok(true),
        )
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Ok(root) = self.root.canonicalize()
            && root.starts_with(&self.parent)
            && root != self.parent
        {
            let _ = fs::remove_dir_all(root);
        }
    }
}

#[derive(Default)]
struct Source {
    pages: VecDeque<NativePage>,
    positions: Vec<Option<NativePosition>>,
    access_checks: usize,
    deny_after: Option<usize>,
    cancel_on_fetch: Option<CancellationToken>,
}
impl PageSource for Source {
    fn check_access(
        &mut self,
        _: &ListBinding,
        control: &InvocationControl,
    ) -> Result<(), CraftError> {
        control.check()?;
        self.access_checks += 1;
        if self
            .deny_after
            .is_some_and(|allowed| self.access_checks > allowed)
        {
            return Err(CraftError::new(
                ErrorCode::PermissionDenied,
                "Fixture access denied.",
            ));
        }
        Ok(())
    }
    fn fetch(
        &mut self,
        _: &ListBinding,
        position: Option<&NativePosition>,
        _: &InvocationControl,
    ) -> Result<NativePage, CraftError> {
        self.positions.push(position.cloned());
        if let Some(cancellation) = &self.cancel_on_fetch {
            cancellation.cancel();
        }
        self.pages
            .pop_front()
            .ok_or_else(|| CraftError::new(ErrorCode::Internal, "Unexpected fixture fetch."))
    }
}
fn position(value: &str) -> NativePosition {
    NativePosition(BTreeMap::from([("cursor".into(), value.into())]))
}
fn page(ids: &[u64], next: Option<&str>) -> NativePage {
    NativePage {
        context: None,
        items: ids.iter().map(|id| json!({"id": id})).collect(),
        next: next.map(position),
    }
}
fn source(pages: Vec<NativePage>) -> Source {
    Source {
        pages: pages.into(),
        ..Source::default()
    }
}
fn handle(page: &ListPage) -> Result<&str, Box<dyn std::error::Error>> {
    page.page
        .next_cursor
        .as_deref()
        .ok_or_else(|| "Missing continuation".into())
}
fn ids(page: &ListPage) -> Vec<u64> {
    page.items
        .iter()
        .filter_map(|item| item["id"].as_u64())
        .collect()
}

#[test]
fn absent_context_preserves_legacy_page_encoding_and_digest() -> TestResult {
    let legacy = json!({"items":[{"id":1}],"next":null});
    let decoded: NativePage = serde_json::from_value(legacy.clone())?;
    assert!(decoded.context.is_none());
    assert_eq!(serde_json::to_value(&decoded)?, legacy);
    let fixture = Fixture::new()?;
    let mut source = source(vec![decoded]);
    let result = fixture.list(None, 10, &mut source)?;
    assert_eq!(result.observation.page_digest, canonical_digest(&legacy)?);
    assert!(serde_json::to_value(result)?.get("context").is_none());
    Ok(())
}

#[test]
fn context_survives_reopen_buffer_replay_and_changes_only_with_native_page() -> TestResult {
    let fixture = Fixture::new()?;
    let mut native = page(&[1, 2, 3], Some("next"));
    native.context = Some(json!({"issue":"10000","status":"1"}));
    let expected_digest = canonical_digest(&serde_json::to_value(&native)?)?;
    let mut empty = page(&[], Some("last"));
    empty.context = Some(json!({"issue":"10000","status":"2"}));
    let mut source = source(vec![native.clone(), empty.clone(), page(&[], None)]);
    let first = fixture.list(None, 1, &mut source)?;
    assert_eq!(first.context, native.context);
    assert_eq!(first.observation.page_digest, expected_digest);
    let old = handle(&first)?;
    let buffered = fixture.list(Some(old), 1, &mut source)?;
    let replay = fixture.list(Some(old), 100, &mut source)?;
    assert_eq!(buffered.context, native.context);
    assert_eq!(replay.context, native.context);
    assert_eq!(replay.observation.page_digest, expected_digest);
    assert!(buffered.cached && replay.cached);
    assert_eq!(source.positions.len(), 1);
    let empty_result = fixture.list(Some(handle(&replay)?), 100, &mut source)?;
    assert!(empty_result.items.is_empty());
    assert_eq!(empty_result.context, empty.context);
    assert_ne!(empty_result.observation.page_digest, expected_digest);
    let last = fixture.list(Some(handle(&empty_result)?), 100, &mut source)?;
    assert!(last.context.is_none());
    assert!(!last.page.has_more);
    assert_eq!(source.positions.len(), 3);
    assert_eq!(source.access_checks, 10);
    Ok(())
}

#[test]
fn oversized_record_recovery_keeps_context_and_rechecks_access() -> TestResult {
    let fixture = Fixture::new()?;
    let context = Some(json!({"issue":"10000","updated":"observed"}));
    let mut source = source(vec![NativePage {
        context: context.clone(),
        items: vec![json!({"id":1,"body":"x".repeat(6000)})],
        next: None,
    }]);
    let first = fixture.store()?.provider_page(
        fixture.request(None, 10),
        &fixture.clock,
        &fixture.control,
        &mut source,
        |page| Ok(page.items.is_empty()),
    )?;
    assert_eq!(first.context, context);
    assert_eq!(
        first.error.as_ref().map(|error| error.code),
        Some(ErrorCode::ItemTooLarge)
    );
    source.deny_after = Some(source.access_checks);
    assert_eq!(
        fixture
            .list(Some(handle(&first)?), 10, &mut source)
            .err()
            .ok_or("Expected access refusal")?
            .code,
        ErrorCode::PermissionDenied
    );
    source.deny_after = None;
    let recovered = fixture.list(Some(handle(&first)?), 10, &mut source)?;
    assert_eq!(recovered.context, context);
    assert_eq!(
        recovered.items[0]["body"]
            .as_str()
            .ok_or("Missing body")?
            .len(),
        6000
    );
    assert!(recovered.cached);
    assert_eq!(source.positions.len(), 1);
    Ok(())
}

#[test]
fn context_has_exact_serialized_object_and_whole_page_bounds() -> TestResult {
    let fixture = Fixture::new()?;
    let exact = json!({"x":"x".repeat(32_760)});
    assert_eq!(serde_json::to_vec(&exact)?.len(), 32_768);
    let mut valid = page(&[], None);
    valid.context = Some(exact.clone());
    assert_eq!(
        fixture.list(None, 10, &mut source(vec![valid]))?.context,
        Some(exact)
    );
    for context in [
        Value::Null,
        json!([]),
        json!("text"),
        json!(true),
        json!({"x":"x".repeat(32_761)}),
    ] {
        let mut native = page(&[1], None);
        native.context = Some(context);
        assert_eq!(
            fixture
                .list(None, 10, &mut source(vec![native]))
                .err()
                .ok_or("Expected context bounds refusal")?
                .code,
            ErrorCode::CursorCapacity
        );
    }
    let too_large = NativePage {
        items: vec![json!({"body":"x".repeat(500_000)})],
        context: Some(json!({"x":"x".repeat(32_760)})),
        next: None,
    };
    assert_eq!(
        fixture
            .list(None, 10, &mut source(vec![too_large]))
            .err()
            .ok_or("Expected whole page bound")?
            .code,
        ErrorCode::CursorCapacity
    );
    Ok(())
}

#[test]
fn legacy_cursor_without_context_resumes_and_invalid_rechecksummed_context_is_refused() -> TestResult
{
    let fixture = Fixture::new()?;
    let mut source = source(vec![page(&[1, 2], None)]);
    let first = fixture.list(None, 1, &mut source)?;
    let cursor = handle(&first)?;
    let raw = fixture.raw()?;
    let payload: String = raw.query_row(
        "SELECT payload FROM provider_cursors WHERE handle=?1",
        [cursor],
        |row| row.get(0),
    )?;
    let original: Value = serde_json::from_str(&payload)?;
    assert!(original.get("context").is_none());
    assert_eq!(ids(&fixture.list(Some(cursor), 10, &mut source)?), vec![2]);
    for value in [json!([]), Value::Null, json!({"x":"x".repeat(32_761)})] {
        let mut altered = original.clone();
        altered["context"] = value;
        let bytes = serde_json::to_string(&altered)?;
        raw.execute(
            "UPDATE provider_cursors SET payload=?1,digest=?2 WHERE handle=?3",
            rusqlite::params![bytes, content_digest(bytes.as_bytes()), cursor],
        )?;
        let before = source.access_checks;
        assert_eq!(
            fixture
                .list(Some(cursor), 10, &mut source)
                .err()
                .ok_or("Expected corrupt context")?
                .code,
            ErrorCode::CursorCorrupt
        );
        assert_eq!(source.access_checks, before);
    }
    assert_eq!(source.positions.len(), 1);
    Ok(())
}

#[test]
fn byte_remainders_replay_without_fetch_and_exhaust_in_order() -> TestResult {
    let fixture = Fixture::new()?;
    let mut source = source(vec![page(&[1, 2, 3, 4], Some("next")), page(&[5], None)]);
    let first = fixture.store()?.provider_page(
        fixture.request(None, 100),
        &fixture.clock,
        &fixture.control,
        &mut source,
        |page| Ok(page.items.len() <= 2),
    )?;
    assert_eq!(ids(&first), vec![1, 2]);
    assert_eq!(first.page.stop_reason, "byte_limit");
    assert!(first.page.truncated);
    let original = handle(&first)?;
    let second = fixture.list(Some(original), 1, &mut source)?;
    let replay = fixture.list(Some(original), 2, &mut source)?;
    assert_eq!(ids(&second), vec![3]);
    assert_eq!(ids(&replay), vec![3, 4]);
    assert!(replay.cached);
    assert_eq!(replay.page.stop_reason, "native_page");
    assert!(!replay.page.truncated);
    assert_eq!(
        first.observation.page_digest,
        replay.observation.page_digest
    );
    assert_eq!(
        first.observation.observed_at,
        replay.observation.observed_at
    );
    assert_eq!(source.positions.len(), 1);
    let last = fixture.list(Some(handle(&replay)?), 100, &mut source)?;
    assert_eq!(ids(&last), vec![5]);
    assert_eq!(last.page.stop_reason, "exhausted");
    assert!(!last.page.has_more);
    assert_eq!(last.page.consistency, "live");
    assert_eq!(source.positions, vec![None, Some(position("next"))]);
    Ok(())
}

#[test]
fn empty_and_short_native_pages_do_not_imply_exhaustion() -> TestResult {
    let fixture = Fixture::new()?;
    let mut source = source(vec![
        page(&[], Some("one")),
        page(&[7], Some("two")),
        page(&[], None),
    ]);
    let first = fixture.list(None, 100, &mut source)?;
    assert!(first.items.is_empty());
    assert!(first.page.has_more);
    assert_eq!(source.positions.len(), 1);
    let second = fixture.list(Some(handle(&first)?), 100, &mut source)?;
    assert_eq!(ids(&second), vec![7]);
    assert_eq!(second.page.stop_reason, "native_page");
    assert_eq!(source.positions.len(), 2);
    let last = fixture.list(Some(handle(&second)?), 100, &mut source)?;
    assert!(!last.page.has_more);
    assert_eq!(source.positions.len(), 3);
    Ok(())
}

#[test]
fn oversized_first_record_retains_position_and_full_envelope_is_bounded() -> TestResult {
    let fixture = Fixture::new()?;
    let huge = json!({"id":1,"body":"x".repeat(5000)});
    let mut source = source(vec![NativePage {
        context: None,
        items: vec![huge.clone(), json!({"id":2})],
        next: None,
    }]);
    let first = fixture.store()?.provider_page(
        fixture.request(None, 100),
        &fixture.clock,
        &fixture.control,
        &mut source,
        |page| {
            let mut envelope = if let Some(error) = &page.error {
                Envelope::failure("jira.project.list", "request-one".into(), error.clone())
            } else {
                Envelope::success(
                    "jira.project.list",
                    "request-one".into(),
                    json!({"items":page.items,"observation":page.observation,"cached":page.cached}),
                )
            };
            envelope.page = Some(page.page.clone());
            Ok(serde_json::to_vec(&envelope)
                .map_err(|_| CraftError::new(ErrorCode::Internal, "Fixture encoding failed."))?
                .len()
                <= 2048)
        },
    )?;
    assert_eq!(
        first.error.as_ref().map(|error| error.code),
        Some(ErrorCode::ItemTooLarge)
    );
    assert!(first.items.is_empty());
    assert_eq!(first.page.stop_reason, "item_too_large");
    let recovered = fixture.list(Some(handle(&first)?), 1, &mut source)?;
    assert_eq!(recovered.items, vec![huge]);
    assert!(recovered.cached);
    let last = fixture.list(Some(handle(&recovered)?), 1, &mut source)?;
    assert_eq!(ids(&last), vec![2]);
    assert_eq!(source.positions.len(), 1);
    Ok(())
}

#[test]
fn invalid_input_or_unfittable_recovery_envelope_sends_no_fetch() -> TestResult {
    let fixture = Fixture::new()?;
    let mut source = Source::default();
    for limit in [0, 101] {
        assert_eq!(
            fixture
                .list(None, limit, &mut source)
                .err()
                .ok_or("Expected invalid limit")?
                .code,
            ErrorCode::InputInvalid
        );
    }
    assert_eq!(
        fixture
            .list(Some("invalid"), 10, &mut source)
            .err()
            .ok_or("Expected invalid cursor")?
            .code,
        ErrorCode::CursorInvalid
    );
    assert_eq!(
        fixture
            .store()?
            .provider_page(
                fixture.request(None, 10),
                &fixture.clock,
                &fixture.control,
                &mut source,
                |_| Ok(false)
            )
            .err()
            .ok_or("Expected preflight refusal")?
            .code,
        ErrorCode::OutputBudgetTooSmall
    );
    assert_eq!(source.access_checks, 0);
    assert!(source.positions.is_empty());
    Ok(())
}

#[test]
fn semantic_binding_changes_refuse_before_source_access() -> TestResult {
    let fixture = Fixture::new()?;
    let mut source = source(vec![page(&[1, 2], None)]);
    let first = fixture.list(None, 1, &mut source)?;
    let before = source.access_checks;
    for field in [
        "operation",
        "descriptor",
        "schema",
        "projection",
        "api",
        "tenant",
        "resource",
        "grant",
        "actor",
        "query",
    ] {
        let mut binding = fixture.binding.clone();
        let other = canonical_digest(&json!({"other":true}))?;
        match field {
            "operation" => binding.operation = "jira.other.list".into(),
            "descriptor" => binding.descriptor_digest = other,
            "schema" => binding.schema_digest = other,
            "projection" => binding.projection_digest = other,
            "api" => binding.api_revision = "v2".into(),
            "tenant" => binding.target.tenant = "tenant-two".into(),
            "resource" => binding.target.id = "other".into(),
            "grant" => binding.grant.id = "gr_00000000000000000000000000000000".into(),
            "actor" => binding.actor = "actor-two".into(),
            _ => {
                binding.query.insert("filter".into(), json!("other"));
            }
        }
        let request = ListRequest {
            binding: &binding,
            cursor: Some(handle(&first)?),
            limit: 2,
        };
        let error = fixture
            .store()?
            .provider_page(
                request,
                &fixture.clock,
                &fixture.control,
                &mut source,
                |_| Ok(true),
            )
            .err()
            .ok_or("Expected binding refusal")?;
        assert_eq!(error.code, ErrorCode::CursorMismatch, "{field}");
    }
    assert_eq!(source.access_checks, before);
    assert_eq!(source.positions.len(), 1);
    Ok(())
}

#[test]
fn revoked_access_blocks_buffer_and_late_revocation_blocks_publication() -> TestResult {
    let fixture = Fixture::new()?;
    let mut source = source(vec![page(&[1, 2], None)]);
    let first = fixture.list(None, 1, &mut source)?;
    source.deny_after = Some(source.access_checks);
    assert_eq!(
        fixture
            .list(Some(handle(&first)?), 1, &mut source)
            .err()
            .ok_or("Expected access denial")?
            .code,
        ErrorCode::PermissionDenied
    );
    assert_eq!(source.positions.len(), 1);
    let mut late = Source {
        deny_after: Some(1),
        ..self::source(vec![page(&[3, 4], None)])
    };
    assert_eq!(
        fixture
            .list(None, 1, &mut late)
            .err()
            .ok_or("Expected late denial")?
            .code,
        ErrorCode::PermissionDenied
    );
    assert_eq!(
        fixture
            .raw()?
            .query_row("SELECT count(*) FROM provider_cursors", [], |row| row
                .get::<_, i64>(0))?,
        1
    );
    Ok(())
}

#[test]
fn cycles_and_repeated_positions_stop_without_a_successor() -> TestResult {
    let fixture = Fixture::new()?;
    let mut source = source(vec![
        page(&[], Some("one")),
        page(&[], Some("two")),
        page(&[9], Some("one")),
    ]);
    let first = fixture.list(None, 10, &mut source)?;
    let second = fixture.list(Some(handle(&first)?), 10, &mut source)?;
    assert_eq!(
        fixture
            .list(Some(handle(&second)?), 10, &mut source)
            .err()
            .ok_or("Expected cycle refusal")?
            .code,
        ErrorCode::PaginationStalled
    );
    assert_eq!(source.positions.len(), 3);
    assert_eq!(
        fixture
            .raw()?
            .query_row("SELECT count(*) FROM provider_cursors", [], |row| row
                .get::<_, i64>(0))?,
        2
    );
    Ok(())
}

#[test]
fn persistence_failure_and_cancellation_do_not_consume_predecessor() -> TestResult {
    let fixture = Fixture::new()?;
    let mut source = source(vec![page(&[1, 2, 3], None)]);
    let first = fixture.list(None, 1, &mut source)?;
    let connection = fixture.raw()?;
    // Keep the already-open trusted store to exercise the write fault, not schema-open refusal.
    let store = fixture.store()?;
    connection.execute_batch("CREATE TRIGGER refuse_cursor BEFORE INSERT ON provider_cursors BEGIN SELECT RAISE(ABORT,'fixture interruption'); END;")?;
    assert!(
        store
            .provider_page(
                fixture.request(Some(handle(&first)?), 1),
                &fixture.clock,
                &fixture.control,
                &mut source,
                |_| Ok(true)
            )
            .is_err()
    );
    connection.execute_batch("DROP TRIGGER refuse_cursor;")?;
    let retry = fixture.list(Some(handle(&first)?), 2, &mut source)?;
    assert_eq!(ids(&retry), vec![2, 3]);
    let mut cancelled = Source {
        cancel_on_fetch: Some(fixture.cancellation.clone()),
        ..self::source(vec![page(&[8, 9], None)])
    };
    assert_eq!(
        fixture
            .list(None, 1, &mut cancelled)
            .err()
            .ok_or("Expected cancellation")?
            .code,
        ErrorCode::Cancelled
    );
    Ok(())
}

#[test]
fn expired_missing_foreign_and_corrupt_payloads_never_restart() -> TestResult {
    let fixture = Fixture::new()?;
    let mut source = source(vec![page(&[1, 2], None)]);
    let first = fixture.list(None, 1, &mut source)?;
    let cursor = handle(&first)?;
    let other = Fixture::new()?;
    assert_eq!(
        other
            .list(Some(cursor), 1, &mut source)
            .err()
            .ok_or("Expected missing cursor")?
            .code,
        ErrorCode::CursorUnavailable
    );
    let raw = fixture.raw()?;
    let (payload, digest): (String, String) = raw.query_row(
        "SELECT payload,digest FROM provider_cursors WHERE handle=?1",
        [cursor],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let mut corrupt: Value = serde_json::from_str(&payload)?;
    corrupt["installation_id"] = json!("ins_00000000000000000000000000000000");
    let foreign = serde_json::to_string(&corrupt)?;
    raw.execute(
        "UPDATE provider_cursors SET payload=?1,digest=?2 WHERE handle=?3",
        rusqlite::params![foreign, content_digest(foreign.as_bytes()), cursor],
    )?;
    assert_eq!(
        fixture
            .list(Some(cursor), 1, &mut source)
            .err()
            .ok_or("Expected foreign installation")?
            .code,
        ErrorCode::CursorUnavailable
    );
    raw.execute(
        "UPDATE provider_cursors SET payload='{}' WHERE handle=?1",
        [cursor],
    )?;
    assert_eq!(
        fixture
            .list(Some(cursor), 1, &mut source)
            .err()
            .ok_or("Expected corrupt cursor")?
            .code,
        ErrorCode::CursorCorrupt
    );
    raw.execute(
        "UPDATE provider_cursors SET payload=?1,digest=?2 WHERE handle=?3",
        rusqlite::params![payload, digest, cursor],
    )?;
    fixture.clock.set_wall(910_000)?;
    assert_eq!(
        fixture
            .list(Some(cursor), 1, &mut source)
            .err()
            .ok_or("Expected expired cursor")?
            .code,
        ErrorCode::CursorExpired
    );
    assert_eq!(source.positions.len(), 1);
    Ok(())
}

#[test]
fn byte_bounded_traversal_preserves_every_id_across_many_small_items_and_empty_pages() -> TestResult
{
    let fixture = Fixture::new()?;
    let first_ids: Vec<_> = (0..1000).collect();
    let mut source = source(vec![
        page(&first_ids, Some("one")),
        page(&[1000, 1001, 1002], Some("two")),
        page(&[], Some("three")),
        page(&[1003, 1004, 1005, 1006, 1007, 1008, 1009], None),
    ]);
    let mut cursor = None;
    let mut emitted = Vec::new();
    let mut pages = 0;
    loop {
        let page = fixture.store()?.provider_page(
            fixture.request(cursor.as_deref(), 100),
            &fixture.clock,
            &fixture.control,
            &mut source,
            |page| {
                Ok(serde_json::to_vec(page)
                    .map_err(|_| CraftError::new(ErrorCode::Internal, "Fixture encoding failed."))?
                    .len()
                    <= 1280)
            },
        )?;
        assert!(page.error.is_none());
        assert!(serde_json::to_vec(&page)?.len() <= 1280);
        emitted.extend(ids(&page));
        pages += 1;
        assert!(pages < 100);
        if !page.page.has_more {
            break;
        }
        cursor = page.page.next_cursor;
    }
    assert_eq!(emitted, (0..1010).collect::<Vec<_>>());
    assert_eq!(source.positions.len(), 4);
    assert!(pages > 4);
    Ok(())
}

#[test]
fn replaying_a_native_position_can_observe_new_live_data_without_consuming_the_cursor() -> TestResult
{
    let fixture = Fixture::new()?;
    let mut source = source(vec![
        page(&[], Some("next")),
        page(&[1], None),
        page(&[2], None),
    ]);
    let first = fixture.list(None, 10, &mut source)?;
    let left = fixture.list(Some(handle(&first)?), 10, &mut source)?;
    let right = fixture.list(Some(handle(&first)?), 10, &mut source)?;
    assert_eq!(ids(&left), vec![1]);
    assert_eq!(ids(&right), vec![2]);
    assert_eq!(
        source.positions,
        vec![None, Some(position("next")), Some(position("next"))]
    );
    assert_eq!(left.page.consistency, "live");
    assert!(!left.cached && !right.cached);
    Ok(())
}

#[test]
fn expiry_before_publication_refuses_results_without_renewing_the_traversal() -> TestResult {
    let fixture = Fixture::new()?;
    let mut source = source(vec![page(&[1, 2], None)]);
    let mut calls = 0;
    let error = fixture
        .store()?
        .provider_page(
            fixture.request(None, 1),
            &fixture.clock,
            &fixture.control,
            &mut source,
            |_| {
                calls += 1;
                if calls == 2 {
                    fixture.clock.set_wall(910_000).map_err(|_| {
                        CraftError::new(ErrorCode::Internal, "Fixture clock failed.")
                    })?;
                }
                Ok(true)
            },
        )
        .err()
        .ok_or("Expected expiry before publication")?;
    assert_eq!(error.code, ErrorCode::CursorExpired);
    assert_eq!(
        fixture
            .raw()?
            .query_row("SELECT count(*) FROM provider_cursors", [], |row| row
                .get::<_, i64>(0))?,
        0
    );
    Ok(())
}

#[test]
fn concurrent_replay_has_independent_successors_and_identical_records() -> TestResult {
    let fixture = Fixture::new()?;
    let mut initial = source(vec![page(&[1, 2, 3, 4], None)]);
    let first = fixture.list(None, 1, &mut initial)?;
    let cursor = handle(&first)?.to_owned();
    let stores = [fixture.store()?, fixture.store()?];
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let results = std::thread::scope(|scope| {
        let mut workers = Vec::new();
        for store in stores {
            let barrier = barrier.clone();
            let request = fixture.request(Some(&cursor), 1);
            let clock = &fixture.clock;
            let control = &fixture.control;
            workers.push(scope.spawn(move || {
                barrier.wait();
                let mut source = Source::default();
                let result =
                    store.provider_page(request, clock, control, &mut source, |_| Ok(true));
                assert!(source.positions.is_empty());
                result
            }));
        }
        workers
            .into_iter()
            .map(|worker| worker.join().map_err(|_| "Cursor worker panicked"))
            .collect::<Result<Vec<_>, _>>()
    })
    .map_err(|error| error.to_string())?;
    let mut handles = Vec::new();
    for result in results {
        let page = result?;
        assert_eq!(ids(&page), vec![2]);
        handles.push(handle(&page)?.to_owned());
        assert_eq!(
            ids(&fixture.list(Some(handle(&page)?), 10, &mut Source::default())?),
            vec![3, 4]
        );
    }
    assert_ne!(handles[0], handles[1]);
    assert_eq!(
        ids(&fixture.list(Some(&cursor), 10, &mut Source::default())?),
        vec![2, 3, 4]
    );
    Ok(())
}

#[test]
fn handle_capacity_refuses_without_eviction_and_expired_rows_are_reclaimed() -> TestResult {
    let fixture = Fixture::new()?;
    let mut initial = source(vec![page(&[1, 2, 3], None)]);
    let first = fixture.list(None, 1, &mut initial)?;
    let raw = fixture.raw()?;
    let (expires, digest, payload): (i64, String, String) = raw.query_row(
        "SELECT expires,digest,payload FROM provider_cursors",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    let transaction = raw.unchecked_transaction()?;
    for index in 0..255 {
        transaction.execute(
            "INSERT INTO provider_cursors(handle,expires,digest,payload) VALUES(?1,?2,?3,?4)",
            rusqlite::params![format!("cur_{index:032x}"), expires, digest, payload],
        )?;
    }
    transaction.commit()?;
    assert_eq!(
        fixture
            .list(Some(handle(&first)?), 1, &mut Source::default())
            .err()
            .ok_or("Expected capacity refusal")?
            .code,
        ErrorCode::CursorCapacity
    );
    assert_eq!(
        raw.query_row("SELECT count(*) FROM provider_cursors", [], |row| row
            .get::<_, i64>(0))?,
        256
    );
    assert_eq!(
        ids(&fixture.list(Some(handle(&first)?), 10, &mut Source::default())?),
        vec![2, 3]
    );
    fixture.clock.set_wall(910_000)?;
    let fresh = fixture.list(None, 1, &mut source(vec![page(&[4, 5], None)]))?;
    assert_eq!(ids(&fresh), vec![4]);
    assert_eq!(
        raw.query_row("SELECT count(*) FROM provider_cursors", [], |row| row
            .get::<_, i64>(0))?,
        1
    );
    assert_eq!(
        fixture
            .list(Some(handle(&first)?), 10, &mut Source::default())
            .err()
            .ok_or("Expected reclaimed cursor refusal")?
            .code,
        ErrorCode::CursorUnavailable
    );
    Ok(())
}

#[test]
fn aggregate_payload_capacity_is_atomic_and_keeps_existing_handles() -> TestResult {
    let fixture = Fixture::new()?;
    let mut initial = source(vec![NativePage {
        context: None,
        items: vec![
            json!({"id":1}),
            json!({"id":2,"body":"x".repeat(350_000)}),
            json!({"id":3}),
        ],
        next: None,
    }]);
    let first = fixture.list(None, 1, &mut initial)?;
    let raw = fixture.raw()?;
    let (expires, digest, payload): (i64, String, String) = raw.query_row(
        "SELECT expires,digest,payload FROM provider_cursors",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    let transaction = raw.unchecked_transaction()?;
    let mut total = payload.len();
    let mut index = 0;
    while total + payload.len() <= 16_777_216 {
        transaction.execute(
            "INSERT INTO provider_cursors(handle,expires,digest,payload) VALUES(?1,?2,?3,?4)",
            rusqlite::params![format!("cur_{index:032x}"), expires, digest, payload],
        )?;
        total += payload.len();
        index += 1;
    }
    transaction.commit()?;
    let count: i64 = raw.query_row("SELECT count(*) FROM provider_cursors", [], |row| {
        row.get(0)
    })?;
    let mut source = Source::default();
    let error = fixture
        .store()?
        .provider_page(
            fixture.request(Some(handle(&first)?), 1),
            &fixture.clock,
            &fixture.control,
            &mut source,
            |page| Ok(page.items.is_empty()),
        )
        .err()
        .ok_or("Expected aggregate capacity refusal")?;
    assert_eq!(error.code, ErrorCode::CursorCapacity);
    assert_eq!(
        raw.query_row("SELECT count(*) FROM provider_cursors", [], |row| row
            .get::<_, i64>(0))?,
        count
    );
    assert_eq!(
        ids(&fixture.list(Some(handle(&first)?), 10, &mut source)?),
        vec![2, 3]
    );
    Ok(())
}

#[test]
fn callback_failures_and_cancellation_after_fit_publish_no_successor() -> TestResult {
    let fixture = Fixture::new()?;
    let mut initial = source(vec![page(&[1, 2, 3], None)]);
    let first = fixture.list(None, 1, &mut initial)?;
    let mut calls = 0;
    let error = fixture
        .store()?
        .provider_page(
            fixture.request(Some(handle(&first)?), 1),
            &fixture.clock,
            &fixture.control,
            &mut Source::default(),
            |_| {
                calls += 1;
                if calls > 1 {
                    Err(CraftError::new(
                        ErrorCode::Internal,
                        "Fixture measurement failed.",
                    ))
                } else {
                    Ok(true)
                }
            },
        )
        .err()
        .ok_or("Expected callback failure")?;
    assert_eq!(error.code, ErrorCode::Internal);
    assert_eq!(
        ids(&fixture.list(Some(handle(&first)?), 10, &mut Source::default())?),
        vec![2, 3]
    );
    let mut calls = 0;
    let error = fixture
        .store()?
        .provider_page(
            fixture.request(Some(handle(&first)?), 1),
            &fixture.clock,
            &fixture.control,
            &mut Source::default(),
            |_| {
                calls += 1;
                if calls > 1 {
                    fixture.cancellation.cancel();
                }
                Ok(true)
            },
        )
        .err()
        .ok_or("Expected cancellation after fit")?;
    assert_eq!(error.code, ErrorCode::Cancelled);
    assert_eq!(
        fixture
            .raw()?
            .query_row("SELECT count(*) FROM provider_cursors", [], |row| row
                .get::<_, i64>(0))?,
        1
    );
    Ok(())
}

#[test]
fn native_page_bounds_and_invalid_continuation_never_publish_partial_items() -> TestResult {
    for bad in [
        NativePage {
            context: None,
            items: vec![json!({"id":1}); 1001],
            next: None,
        },
        NativePage {
            context: None,
            items: vec![json!({"id":1,"body":"x".repeat(524_288)})],
            next: None,
        },
        NativePage {
            context: None,
            items: vec![json!({"id":1})],
            next: Some(NativePosition(BTreeMap::from([(
                "authorization".into(),
                "private-sentinel".into(),
            )]))),
        },
    ] {
        let fixture = Fixture::new()?;
        let mut source = source(vec![bad]);
        let error = fixture
            .list(None, 10, &mut source)
            .err()
            .ok_or("Expected provider bounds refusal")?;
        assert!(matches!(
            error.code,
            ErrorCode::CursorCapacity | ErrorCode::RemoteUnavailable
        ));
        assert!(!serde_json::to_string(&error)?.contains("private-sentinel"));
        assert_eq!(
            fixture
                .raw()?
                .query_row("SELECT count(*) FROM provider_cursors", [], |row| row
                    .get::<_, i64>(0))?,
            0
        );
    }
    Ok(())
}

#[test]
fn semantic_cursor_corruption_is_refused_even_with_a_recomputed_checksum() -> TestResult {
    let fixture = Fixture::new()?;
    let mut initial = source(vec![page(&[1, 2], Some("next"))]);
    let first = fixture.list(None, 1, &mut initial)?;
    let raw = fixture.raw()?;
    let payload: String =
        raw.query_row("SELECT payload FROM provider_cursors", [], |row| row.get(0))?;
    let original: Value = serde_json::from_str(&payload)?;
    for field in [
        "unknown",
        "version",
        "observation",
        "history",
        "items",
        "next",
    ] {
        let mut changed = original.clone();
        match field {
            "unknown" => changed["extra"] = json!("private-sentinel"),
            "version" => changed["format_version"] = json!(99),
            "observation" => changed["observation"]["observed_at"] = json!(9_999),
            "history" => changed["seen"] = json!([original["seen"][0], original["seen"][0]]),
            "items" => changed["items"] = json!(["private-sentinel"]),
            _ => changed["next"] = json!({"cursor":"private-sentinel\n"}),
        }
        let payload = serde_json::to_string(&changed)?;
        raw.execute(
            "UPDATE provider_cursors SET payload=?1,digest=?2",
            rusqlite::params![payload, content_digest(payload.as_bytes())],
        )?;
        let error = fixture
            .list(Some(handle(&first)?), 10, &mut Source::default())
            .err()
            .ok_or("Expected semantic corruption refusal")?;
        assert_eq!(error.code, ErrorCode::CursorCorrupt, "{field}");
        assert!(!serde_json::to_string(&error)?.contains("private-sentinel"));
    }
    Ok(())
}

#[test]
fn fixed_traversal_expiry_is_not_extended_by_buffered_or_native_resume() -> TestResult {
    let fixture = Fixture::new()?;
    let mut source = source(vec![page(&[1, 2], Some("next")), page(&[3, 4], None)]);
    let first = fixture.list(None, 1, &mut source)?;
    fixture.clock.set_wall(800_000)?;
    let buffered = fixture.list(Some(handle(&first)?), 1, &mut source)?;
    let next = fixture.list(Some(handle(&buffered)?), 1, &mut source)?;
    assert_eq!(next.observation.observed_at, 800_000);
    assert!(!next.cached);
    fixture.clock.set_wall(910_000)?;
    assert_eq!(
        fixture
            .list(Some(handle(&next)?), 10, &mut source)
            .err()
            .ok_or("Expected original expiry")?
            .code,
        ErrorCode::CursorExpired
    );
    assert_eq!(source.positions.len(), 2);
    Ok(())
}

#[test]
fn provider_cursor_schema_migration_preserves_existing_state_and_rolls_back_conflicts() -> TestResult
{
    use craft_core::profiles::ProfileInput;
    use craft_core::storage::{OperationBinding, OperationJournal};
    use craft_runtime::create_policy::CreatePolicyUpdate;
    use craft_runtime::journal::NativeJournal;
    use craft_runtime::profile_pages::ProfileListRequest;

    for version in [1, 2, 3, 8, 9, 10] {
        let fixture = Fixture::new()?;
        let store = fixture.store()?;
        let settings = store.settings().clone();
        let profile = store.get("factory", &fixture.control)?;
        store.use_profile("factory", &fixture.control, |_| Ok(()))?;
        store.create(
            &ProfileInput {
                name: "second".into(),
                provider: "jira".into(),
                deployment: "cloud".into(),
                auth_mode: AuthMode::Service,
                grant: Some(profile.grant.clone()),
                tenant: profile.tenant.clone(),
                defaults: BTreeMap::new(),
            },
            &fixture.control,
            |_| Ok(()),
        )?;
        let local = store.list_page(
            ProfileListRequest {
                cursor: None,
                binding: "profile-list",
                limit: 1,
            },
            &fixture.clock,
            &fixture.control,
            |_| Ok(true),
        )?;
        let local_cursor = local
            .page
            .next_cursor
            .as_deref()
            .ok_or("Missing local cursor")?;
        let target = ResourceRef {
            resource_type: "project".into(),
            id: "10000".into(),
            ..fixture.binding.target.clone()
        };
        let policy = store.set_create_policy(
            "factory",
            CreatePolicyUpdate {
                target: target.clone(),
                expected_actor: fixture.binding.actor.clone(),
                enabled: false,
                expected_revision: None,
            },
            true,
            &fixture.control,
            |_| Ok(()),
        )?;
        let recovery = store.set_issue_recovery_policy(
            "factory",
            craft_runtime::issue_recovery_policy::IssueRecoveryPolicyUpdate {
                target: target.clone(),
                expected_actor: fixture.binding.actor.clone(),
                enabled: false,
                operations: vec![
                    craft_runtime::issue_recovery_policy::IssueRecoveryOperation::Create,
                ],
                expected_revision: None,
            },
            true,
            &fixture.control,
            |_| Ok(()),
        )?;
        let journal = NativeJournal::open(&store, &fixture.control)?;
        let operation = journal.begin(&OperationBinding {
            operation_id: "create-report".into(),
            request_digest: fixture.binding.descriptor_digest.clone(),
            descriptor_digest: fixture.binding.descriptor_digest.clone(),
            target,
            grant: profile.grant.clone(),
            actor: fixture.binding.actor.clone(),
        })?;
        let mut provider_source = source(vec![page(&[1, 2, 3], Some("next"))]);
        let provider_page = fixture.list(None, 1, &mut provider_source)?;
        let provider_cursor = handle(&provider_page)?.to_owned();
        drop(journal);
        drop(store);
        let raw = fixture.raw()?;
        raw.execute_batch(
            "DROP TABLE IF EXISTS provider_policies; DROP TABLE IF EXISTS confluence_admin_policies; DROP TABLE IF EXISTS review_policies; DROP TABLE confluence_write_policies;",
        )?;
        if version < 10 {
            raw.execute_batch("DROP TABLE IF EXISTS provider_policies; DROP TABLE IF EXISTS confluence_admin_policies; DROP TABLE confluence_read_policies;")?;
        }
        if version < 9 {
            raw.execute_batch("DROP TABLE IF EXISTS provider_policies; DROP TABLE IF EXISTS confluence_admin_policies; DROP TABLE issue_recovery_policies;")?;
        }
        if version < 8 {
            raw.execute_batch("DROP TABLE IF EXISTS provider_policies; DROP TABLE IF EXISTS confluence_admin_policies; DROP TABLE issue_mutation_policies; DROP TABLE field_read_policies; DROP TABLE issue_read_policies; DROP TABLE project_read_policies; DROP TABLE provider_cursors;")?;
        }
        if version < 3 {
            raw.execute_batch(
                "DROP TABLE IF EXISTS provider_policies; DROP TABLE IF EXISTS confluence_admin_policies; DROP TABLE create_policies;",
            )?;
        }
        if version < 2 {
            raw.execute_batch(
                "DROP TABLE IF EXISTS provider_policies; DROP TABLE IF EXISTS confluence_admin_policies; DROP TABLE operation_records;",
            )?;
        }
        raw.pragma_update(None, "user_version", version)?;
        let migrated = fixture.store()?;
        assert_eq!(migrated.settings(), &settings);
        assert_eq!(migrated.get("factory", &fixture.control)?, profile);
        assert_eq!(
            migrated.active(&fixture.control)?.as_deref(),
            Some("factory")
        );
        assert_eq!(
            migrated
                .list_page(
                    ProfileListRequest {
                        cursor: Some(local_cursor),
                        binding: "profile-list",
                        limit: 1
                    },
                    &fixture.clock,
                    &fixture.control,
                    |_| Ok(true)
                )?
                .items[0]
                .name,
            "second"
        );
        if version >= 3 {
            assert_eq!(
                migrated.create_policy("factory", &fixture.control)?,
                Some(policy)
            );
        }
        if version >= 2 {
            assert_eq!(
                NativeJournal::open(&migrated, &fixture.control)?.get("create-report")?,
                Some(operation)
            );
        }
        if version >= 8 {
            let buffered = migrated.provider_page(
                fixture.request(Some(&provider_cursor), 1),
                &fixture.clock,
                &fixture.control,
                &mut provider_source,
                |_| Ok(true),
            )?;
            assert_eq!(ids(&buffered), vec![2]);
            assert_eq!(provider_source.positions.len(), 1);
            assert_eq!(
                migrated.issue_recovery_policy("factory", "10000", &fixture.control)?,
                if version >= 9 { Some(recovery) } else { None }
            );
            assert!(
                migrated
                    .confluence_read_policy("factory", &fixture.control)?
                    .is_none()
            );
        }
        assert_eq!(
            raw.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))?,
            14
        );
        drop(migrated);
        raw.execute_batch("DROP TABLE IF EXISTS provider_policies; DROP TABLE IF EXISTS confluence_admin_policies; DROP TABLE IF EXISTS review_policies; DROP TABLE confluence_write_policies; DROP TABLE confluence_read_policies; DROP TABLE issue_recovery_policies; DROP TABLE issue_mutation_policies; DROP TABLE field_read_policies; DROP TABLE issue_read_policies; DROP TABLE project_read_policies; DROP TABLE provider_cursors; DROP TABLE create_policies; DROP TABLE operation_records; CREATE TABLE provider_cursors(foreign_data TEXT); PRAGMA user_version=1;")?;
        assert!(fixture.store().is_err());
        assert_eq!(
            raw.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))?,
            1
        );
        assert_eq!(raw.query_row("SELECT count(*) FROM sqlite_schema WHERE name IN ('operation_records','create_policies')",[],|row|row.get::<_,i64>(0))?,0);
        assert_eq!(
            raw.query_row(
                "SELECT sql FROM sqlite_schema WHERE name='provider_cursors'",
                [],
                |row| row.get::<_, String>(0)
            )?,
            "CREATE TABLE provider_cursors(foreign_data TEXT)"
        );
    }
    let fixture = Fixture::new()?;
    fixture.raw()?.execute_batch(
        "CREATE TRIGGER altered_cursor AFTER INSERT ON provider_cursors BEGIN SELECT 1; END;",
    )?;
    assert!(fixture.store().is_err());
    Ok(())
}
