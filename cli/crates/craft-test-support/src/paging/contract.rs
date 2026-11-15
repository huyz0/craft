//! A reusable paging conformance contract for any list adapter behind `PageSource`.
//!
//! The contract observes only emitted IDs, page metadata and native fetch counts, so the same
//! cases apply to a synthetic service or to a real provider adapter over simulated wire
//! responses. Live changes between pages are recorded as declared limitations, never as
//! failures, and no case accepts a claim of snapshot consistency.
use std::collections::BTreeMap;

use craft_core::error::{CraftError, ErrorCode};
use craft_core::pagination::{ListBinding, ListPage, PageSource};

use super::{Fault, Style};

/// A page source that also reports how many native pages it fetched.
pub trait CountingSource: PageSource {
    fn fetches(&self) -> usize;
}

/// One list adapter under test.
pub trait ConformanceSubject {
    fn name(&self) -> String;
    fn style(&self) -> Style;
    /// A fresh source over `records` native records served `page_size` at a time.
    fn source(
        &self,
        records: usize,
        page_size: usize,
        faults: BTreeMap<usize, Fault>,
    ) -> Box<dyn CountingSource + '_>;
    fn binding(&self) -> ListBinding;
    /// The same traversal with one semantic change (query, target or projection).
    fn changed_binding(&self) -> ListBinding;
    /// Normalized IDs, in native order, of a fresh dataset of `records` records.
    fn expected_ids(&self, records: usize) -> Vec<String>;
    fn id(&self, item: &serde_json::Value) -> Option<String>;
    /// Whether the native wire can advertise a position the client already requested.
    fn can_repeat_position(&self) -> bool;
}

/// The interactive cursor engine under its real persistence, supplied by the runtime.
pub trait Pager {
    fn page(
        &mut self,
        binding: &ListBinding,
        cursor: Option<&str>,
        limit: u16,
        max_bytes: usize,
        source: &mut dyn PageSource,
    ) -> Result<ListPage, CraftError>;
    /// Serialized size of the engine's fixed recovery envelope, the smallest usable budget.
    fn recovery_bytes(&self) -> usize;
}

#[derive(Debug, Default)]
pub struct Report {
    pub cases: Vec<String>,
    /// Observed live-traversal effects: (case, duplicate IDs, missing IDs).
    pub live_limitations: Vec<(String, usize, usize)>,
}

fn fail(subject: &dyn ConformanceSubject, case: &str, detail: impl std::fmt::Display) -> String {
    format!("{} / {case}: {detail}", subject.name())
}

fn size(page: &ListPage) -> usize {
    serde_json::to_vec(page).map_or(usize::MAX, |bytes| bytes.len())
}

struct Traversal {
    ids: Vec<String>,
    pages: Vec<ListPage>,
    fetches_per_call: Vec<usize>,
}

fn traverse(
    subject: &dyn ConformanceSubject,
    pager: &mut dyn Pager,
    source: &mut dyn CountingSource,
    binding: &ListBinding,
    limit: u16,
    max_bytes: usize,
) -> Result<Traversal, String> {
    let mut cursor: Option<String> = None;
    let mut result = Traversal {
        ids: Vec::new(),
        pages: Vec::new(),
        fetches_per_call: Vec::new(),
    };
    for _ in 0..10_000 {
        let before = source.fetches();
        let page = pager
            .page(binding, cursor.as_deref(), limit, max_bytes, source)
            .map_err(|error| {
                fail(
                    subject,
                    "traverse",
                    format!("{:?}: {}", error.code, error.message),
                )
            })?;
        result.fetches_per_call.push(source.fetches() - before);
        if page.page.consistency != "live" {
            return Err(fail(subject, "traverse", "claimed non-live consistency"));
        }
        if page.page.returned != page.items.len() {
            return Err(fail(
                subject,
                "traverse",
                "returned count differs from items",
            ));
        }
        result
            .ids
            .extend(page.items.iter().filter_map(|item| subject.id(item)));
        let next = page.page.next_cursor.clone();
        if page.page.has_more != next.is_some() {
            return Err(fail(
                subject,
                "traverse",
                "has_more disagrees with next_cursor",
            ));
        }
        result.pages.push(page);
        match next {
            Some(next) => cursor = Some(next),
            None => return Ok(result),
        }
    }
    Err(fail(subject, "traverse", "did not terminate"))
}

/// Runs every case. Returns the passed case names and observed live limitations.
pub fn run(subject: &dyn ConformanceSubject, pager: &mut dyn Pager) -> Result<Report, String> {
    let mut report = Report::default();
    let binding = subject.binding();
    // Lossless, duplicate-free traversal of a static dataset at several limits and sizes.
    for (records, page_size, limit) in [
        (0, 5, 3),
        (1, 5, 3),
        (5, 5, 5),
        (11, 5, 3),
        (11, 5, 100),
        (12, 4, 1),
    ] {
        let case = format!("lossless records={records} page={page_size} limit={limit}");
        let mut source = subject.source(records, page_size, BTreeMap::new());
        let traversal = traverse(subject, pager, source.as_mut(), &binding, limit, 1_048_576)?;
        if traversal.ids != subject.expected_ids(records) {
            return Err(fail(subject, &case, format!("ids {:?}", traversal.ids)));
        }
        if traversal.fetches_per_call.iter().any(|count| *count > 1) {
            return Err(fail(
                subject,
                &case,
                "more than one native fetch in an invocation",
            ));
        }
        let pages = records.div_ceil(page_size).max(1);
        if source.fetches() != pages {
            return Err(fail(
                subject,
                &case,
                format!("{} fetches for {pages} native pages", source.fetches()),
            ));
        }
        let last = traversal
            .pages
            .last()
            .ok_or_else(|| fail(subject, &case, "no page"))?;
        if last.page.stop_reason != "exhausted" {
            return Err(fail(subject, &case, "terminal page is not exhausted"));
        }
        report.cases.push(case);
    }
    // Mid-page byte truncation resumes at the first unreturned record.
    {
        let case = "byte truncation".to_string();
        let mut probe = subject.source(30, 10, BTreeMap::new());
        let first = pager
            .page(&binding, None, 100, 1_048_576, probe.as_mut())
            .map_err(|error| fail(subject, &case, error.message))?;
        let full = size(&first);
        let mut three = first.clone();
        three.items.truncate(3);
        let budget = (pager.recovery_bytes() + 16).max(size(&three) + 8);
        if budget >= full {
            return Err(fail(
                subject,
                &case,
                "records too small to observe truncation",
            ));
        }
        let mut source = subject.source(30, 10, BTreeMap::new());
        let traversal = traverse(subject, pager, source.as_mut(), &binding, 100, budget)?;
        if traversal.ids != subject.expected_ids(30) {
            return Err(fail(subject, &case, format!("ids {:?}", traversal.ids)));
        }
        if !traversal
            .pages
            .iter()
            .any(|page| page.page.stop_reason == "byte_limit")
        {
            return Err(fail(subject, &case, "no byte_limit stop observed"));
        }
        if source.fetches() != 3 || traversal.fetches_per_call.iter().any(|count| *count > 1) {
            return Err(fail(
                subject,
                &case,
                "byte truncation refetched a native page",
            ));
        }
        report.cases.push(case);
    }
    // Replaying a buffered cursor returns the same records without a fetch.
    {
        let case = "buffered replay".to_string();
        let mut source = subject.source(11, 5, BTreeMap::new());
        let first = pager
            .page(&binding, None, 2, 1_048_576, source.as_mut())
            .map_err(|error| fail(subject, &case, error.message))?;
        let cursor = first
            .page
            .next_cursor
            .clone()
            .ok_or_else(|| fail(subject, &case, "no cursor"))?;
        let fetched = source.fetches();
        let again = pager
            .page(&binding, Some(&cursor), 2, 1_048_576, source.as_mut())
            .map_err(|error| fail(subject, &case, error.message))?;
        let twice = pager
            .page(&binding, Some(&cursor), 2, 1_048_576, source.as_mut())
            .map_err(|error| fail(subject, &case, error.message))?;
        let ids = |page: &ListPage| {
            page.items
                .iter()
                .filter_map(|item| subject.id(item))
                .collect::<Vec<_>>()
        };
        if ids(&again) != ids(&twice) || source.fetches() != fetched || !again.cached {
            return Err(fail(
                subject,
                &case,
                "replay changed records, fetched or lost cached provenance",
            ));
        }
        if again.observation.page_digest != first.observation.page_digest {
            return Err(fail(subject, &case, "replay lost its original observation"));
        }
        // Changed semantics fail before any fetch.
        let changed = pager.page(
            &subject.changed_binding(),
            Some(&cursor),
            2,
            1_048_576,
            source.as_mut(),
        );
        if changed.as_ref().err().map(|error| error.code) != Some(ErrorCode::CursorMismatch)
            || source.fetches() != fetched
        {
            return Err(fail(
                subject,
                "binding change",
                "changed semantics were not refused before fetch",
            ));
        }
        report.cases.push(case);
        report.cases.push("binding change".into());
    }
    // An oversized first record keeps a recovery cursor at that record.
    {
        let case = "oversized first record".to_string();
        let mut source = subject.source(3, 5, BTreeMap::new());
        let probe = pager
            .page(&binding, None, 1, 1_048_576, source.as_mut())
            .map_err(|error| fail(subject, &case, error.message))?;
        let mut source = subject.source(3, 5, BTreeMap::new());
        // The smallest usable budget: the fixed recovery envelope plus the page's own context.
        let context = probe
            .context
            .as_ref()
            .and_then(|context| serde_json::to_vec(context).ok())
            .map_or(0, |bytes| bytes.len() + 16);
        let budget = pager.recovery_bytes() + context;
        let page = pager
            .page(&binding, None, 1, budget, source.as_mut())
            .map_err(|error| fail(subject, &case, error.message))?;
        if size(&probe) > budget && !page.items.is_empty() {
            return Err(fail(subject, &case, "an oversized record was returned"));
        }
        if page.items.is_empty() {
            if page.error.as_ref().map(|error| error.code) != Some(ErrorCode::ItemTooLarge) {
                return Err(fail(subject, &case, "empty page without ITEM_TOO_LARGE"));
            }
            let cursor = page
                .page
                .next_cursor
                .ok_or_else(|| fail(subject, &case, "no recovery cursor"))?;
            let resumed = pager
                .page(&binding, Some(&cursor), 1, 1_048_576, source.as_mut())
                .map_err(|error| fail(subject, &case, error.message))?;
            if resumed.items.first().and_then(|item| subject.id(item))
                != subject.expected_ids(3).first().cloned()
            {
                return Err(fail(
                    subject,
                    &case,
                    "recovery skipped the oversized record",
                ));
            }
        }
        report.cases.push(case);
    }
    // A repeated native position stops as stalled instead of looping.
    if subject.can_repeat_position() {
        let case = "repeated position".to_string();
        let mut source = subject.source(11, 5, BTreeMap::from([(1, Fault::RepeatPosition)]));
        let mut cursor = None;
        let mut outcome = None;
        for _ in 0..10 {
            match pager.page(&binding, cursor.as_deref(), 100, 1_048_576, source.as_mut()) {
                Ok(page) => match page.page.next_cursor {
                    Some(next) => cursor = Some(next),
                    None => break,
                },
                Err(error) => {
                    outcome = Some(error.code);
                    break;
                }
            }
        }
        if outcome != Some(ErrorCode::PaginationStalled) {
            return Err(fail(subject, &case, format!("{outcome:?}")));
        }
        report.cases.push(case);
    }
    // Live changes between pages: duplicates or gaps are possible and stay labelled live.
    for (fault, label) in [
        (Fault::InsertFront("inserted".into()), "live insertion"),
        (Fault::Delete(String::new()), "live deletion"),
    ] {
        let mut source = subject.source(11, 5, BTreeMap::from([(1, fault)]));
        let traversal = traverse(subject, pager, source.as_mut(), &binding, 100, 1_048_576)?;
        let expected = subject.expected_ids(11);
        let mut seen = std::collections::BTreeSet::new();
        let duplicates = traversal
            .ids
            .iter()
            .filter(|id| !seen.insert((*id).clone()))
            .count();
        let missing = expected
            .iter()
            .filter(|id| !traversal.ids.contains(id))
            .count();
        report
            .live_limitations
            .push((label.into(), duplicates, missing));
        report.cases.push(label.into());
    }
    Ok(report)
}
