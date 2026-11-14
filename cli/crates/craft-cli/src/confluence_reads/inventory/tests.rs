use super::*;
use craft_core::control::CancellationToken;
use std::sync::atomic::{AtomicU64, Ordering};
type TestResult = Result<(), Box<dyn std::error::Error>>;
fn page() -> ResourceRef {
    ResourceRef {
        provider: "confluence".into(),
        deployment: "cloud".into(),
        tenant: "00000000-1111-2222-3333-444444444444".into(),
        resource_type: "page".into(),
        id: "101".into(),
    }
}
fn item(id: &str, parent: Option<&str>, body: &str) -> Result<Value, CraftError> {
    let digest = canonical_digest(
        &json!({"id":id,"page_id":"101","parent_comment_id":parent,"status":"current","version":1,"version_author_id":"reviewer-one","version_created_at":"2026-11-10T00:00:00.000Z","representation":"storage","body":body}),
    )?;
    Ok(
        json!({"id":id,"page_id":"101","parent_comment_id":parent,"status":"current","version":1,"version_author_id":"reviewer-one","version_created_at":"2026-11-10T00:00:00.000Z","body_storage":body,"body_state":"returned","fields_not_returned":[],"revision":{"version":1,"representation":"storage","digest":digest}}),
    )
}
fn position(cursor: &str) -> NativePosition {
    NativePosition(std::collections::BTreeMap::from([(
        "cursor".into(),
        cursor.into(),
    )]))
}
struct Step {
    parent: Option<String>,
    cursor: Option<String>,
    result: Result<NativePage, CraftError>,
}
fn step(parent: Option<&str>, cursor: Option<&str>, items: Vec<Value>, next: Option<&str>) -> Step {
    Step {
        parent: parent.map(String::from),
        cursor: cursor.map(String::from),
        result: Ok(NativePage {
            context: parent.map(|id|json!({"page_id":"101","parent_comment_id":id,"coverage":"direct_replies","parent":{"id":id,"page_id":"101","status":"current","version":1,"version_author_id":"reviewer-one","version_created_at":"2026-11-10T00:00:00.000Z"}})),
            items,
            next: next.map(position),
        }),
    }
}
struct Script {
    steps: VecDeque<Step>,
    reads: usize,
    checks: Cell<usize>,
    deny_at: usize,
}
impl PageReader for Script {
    fn check(&self) -> Result<(), CraftError> {
        let n = self.checks.get() + 1;
        self.checks.set(n);
        if n >= self.deny_at {
            Err(refused("Fixture policy revoked."))
        } else {
            Ok(())
        }
    }
    fn read(
        &mut self,
        parent: Option<&str>,
        cursor: Option<&NativePosition>,
    ) -> Result<NativePage, CraftError> {
        self.reads += 1;
        let step = self.steps.pop_front().ok_or_else(malformed)?;
        assert_eq!(parent, step.parent.as_deref());
        assert_eq!(
            cursor.and_then(|v| v.0.get("cursor").map(String::as_str)),
            step.cursor.as_deref()
        );
        step.result
    }
}
fn script(steps: Vec<Step>) -> Script {
    Script {
        steps: steps.into(),
        reads: 0,
        checks: Cell::new(0),
        deny_at: usize::MAX,
    }
}
#[test]
fn complete_collection_exhausts_empty_root_pages_and_every_nested_leaf() -> TestResult {
    let mut source = script(vec![
        step(None, None, vec![], Some("a")),
        step(None, Some("a"), vec![item("10", None, "root")?], Some("b")),
        step(None, Some("b"), vec![item("20", None, "root two")?], None),
        step(
            Some("10"),
            None,
            vec![item("11", Some("10"), "reply")?],
            None,
        ),
        step(Some("20"), None, vec![], None),
        step(
            Some("11"),
            None,
            vec![item("12", Some("11"), "nested")?],
            None,
        ),
        step(Some("12"), None, vec![], None),
    ]);
    let result = collect(&mut source, &page())?;
    assert!(matches!(result.coverage, Coverage::Complete));
    assert_eq!(source.reads, 7);
    assert!(source.steps.is_empty());
    assert_eq!(
        result
            .comments
            .iter()
            .map(|c| c.id.as_str())
            .collect::<Vec<_>>(),
        vec!["10", "20", "11", "12"]
    );
    Ok(())
}
#[test]
fn gaps_duplicate_membership_and_cyclic_cursors_never_return_complete_coverage() -> TestResult {
    for mode in [
        "unreadable_leaf",
        "duplicate_root",
        "cross_branch",
        "cursor_cycle",
        "revoked",
    ] {
        let mut steps = vec![step(None, None, vec![item("10", None, "root")?], None)];
        match mode {
            "unreadable_leaf" => steps.push(Step {
                parent: Some("10".into()),
                cursor: None,
                result: Err(malformed()),
            }),
            "duplicate_root" => {
                steps = vec![step(
                    None,
                    None,
                    vec![item("10", None, "a")?, item("10", None, "b")?],
                    None,
                )]
            }
            "cross_branch" => steps.push(step(
                Some("10"),
                None,
                vec![item("10", Some("10"), "cycle")?],
                None,
            )),
            "cursor_cycle" => {
                steps = vec![
                    step(None, None, vec![], Some("a")),
                    step(None, Some("a"), vec![], Some("b")),
                    step(None, Some("b"), vec![], Some("a")),
                ]
            }
            _ => {}
        }
        let mut source = script(steps);
        if mode == "revoked" {
            source.deny_at = 3;
        }
        let failure = collect(&mut source, &page())
            .err()
            .ok_or("expected refusal")?;
        if mode == "cursor_cycle" {
            assert_eq!(failure.code, ErrorCode::PaginationStalled);
            assert_eq!(source.reads, 3);
        }
    }
    Ok(())
}
#[test]
fn current_parent_proof_cannot_confirm_a_stale_listed_approval() -> TestResult {
    let mut current = step(Some("10"), None, vec![], None);
    current
        .result
        .as_mut()
        .map_err(|_| "fixture")?
        .context
        .as_mut()
        .ok_or("proof")?["parent"]["version"] = json!(2);
    let mut source = script(vec![
        step(None, None, vec![item("10", None, "old approval")?], None),
        current,
    ]);
    assert_eq!(
        collect(&mut source, &page())
            .err()
            .ok_or("stale list refusal")?
            .code,
        ErrorCode::VersionConflict
    );
    assert_eq!(source.reads, 2);
    Ok(())
}
#[test]
fn incomplete_or_contradictory_normalized_items_cannot_enter_inventory() -> TestResult {
    for mode in [
        "page",
        "parent",
        "missing_parent",
        "author",
        "version",
        "status",
        "body_state",
        "body",
        "fingerprint",
        "missing_revision",
        "unreturned",
        "alternate_body",
    ] {
        let mut raw = item("10", None, "visible")?;
        match mode {
            "page" => raw["page_id"] = json!("999"),
            "parent" => raw["parent_comment_id"] = json!("999"),
            "missing_parent" => {
                raw.as_object_mut()
                    .ok_or("item")?
                    .remove("parent_comment_id");
            }
            "author" => raw["version_author_id"] = json!(""),
            "version" => raw["version"] = json!(0),
            "status" => raw["status"] = json!("historical"),
            "body_state" => raw["body_state"] = json!("not_requested"),
            "body" => raw["body_storage"] = Value::Null,
            "fingerprint" => raw["body_storage"] = json!("edited"),
            "missing_revision" => raw["revision"] = Value::Null,
            "unreturned" => raw["fields_not_returned"] = json!(["body_storage"]),
            _ => raw["body_atlas_doc_format"] = json!("{}"),
        }
        let mut source = script(vec![step(None, None, vec![raw], None)]);
        assert!(collect(&mut source, &page()).is_err(), "{mode}");
        assert_eq!(source.reads, 1);
    }
    Ok(())
}
struct Generated {
    reads: usize,
    width: usize,
    empty: bool,
}
impl PageReader for Generated {
    fn check(&self) -> Result<(), CraftError> {
        Ok(())
    }
    fn read(
        &mut self,
        parent: Option<&str>,
        _: Option<&NativePosition>,
    ) -> Result<NativePage, CraftError> {
        assert!(parent.is_none());
        let start = self.reads * 25 + 1;
        self.reads += 1;
        Ok(NativePage {
            context: None,
            items: if self.empty {
                vec![]
            } else {
                (start..start + 25)
                    .map(|id| item(&id.to_string(), None, &"x".repeat(self.width)))
                    .collect::<Result<Vec<_>, _>>()?
            },
            next: Some(position(&format!("next-{}", self.reads))),
        })
    }
}
#[test]
fn comment_body_total_and_empty_page_limits_bound_native_traversal() {
    for (width, empty, expected_reads) in [
        (1, false, 41),
        (MAX_BODY_BYTES, false, 1),
        (65_000, false, 6),
        (0, true, MAX_PAGES),
    ] {
        let mut source = Generated {
            reads: 0,
            width,
            empty,
        };
        assert!(collect(&mut source, &page()).is_err());
        assert_eq!(source.reads, expected_reads, "width={width} empty={empty}");
    }
}
struct MutableClock {
    wall: AtomicU64,
    mono: AtomicU64,
}
impl Clock for MutableClock {
    fn unix_millis(&self) -> u64 {
        self.wall.load(Ordering::SeqCst)
    }
    fn monotonic_millis(&self) -> u64 {
        self.mono.load(Ordering::SeqCst)
    }
}
#[test]
fn dual_clock_expiry_rollback_and_cancellation_refuse_late_inventory() -> TestResult {
    for mode in [
        "wall_expiry",
        "mono_expiry",
        "wall_rollback",
        "mono_rollback",
        "cancel",
    ] {
        let clock = Arc::new(MutableClock {
            wall: AtomicU64::new(1000),
            mono: AtomicU64::new(1000),
        });
        let cancellation = CancellationToken::default();
        let control = InvocationControl::new(clock.clone(), cancellation.clone(), 60_000)?;
        let budget = Budget::new(clock.clone());
        clock.wall.store(1500, Ordering::SeqCst);
        clock.mono.store(1500, Ordering::SeqCst);
        budget.check(&control)?;
        match mode {
            "wall_expiry" => clock.wall.store(31_000, Ordering::SeqCst),
            "mono_expiry" => clock.mono.store(31_000, Ordering::SeqCst),
            "wall_rollback" => clock.wall.store(1400, Ordering::SeqCst),
            "mono_rollback" => clock.mono.store(1400, Ordering::SeqCst),
            _ => cancellation.cancel(),
        }
        let error = budget.check(&control).err().ok_or("freshness refusal")?;
        assert_eq!(
            error.code,
            if mode == "cancel" {
                ErrorCode::Cancelled
            } else {
                ErrorCode::PolicyDenied
            }
        );
    }
    Ok(())
}
