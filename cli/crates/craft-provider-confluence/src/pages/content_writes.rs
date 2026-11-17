//! Folder and whiteboard create and trash in an allowed space. Create reads the space, and the
//! parent page or folder when one is named; trash rechecks the reviewed version. Purge is not
//! offered, and private creation is not exposed. A whiteboard may start from a built-in
//! template; its drawing content is otherwise only reachable through the editor.
use super::*;
use crate::content::{ContentKind, has_children, observe};
use craft_core::storage::SubmissionState;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const FOLDER_CREATE: &str = "confluence.folder.create";
pub const FOLDER_TRASH: &str = "confluence.folder.trash";
pub const WHITEBOARD_CREATE: &str = "confluence.whiteboard.create";
pub const WHITEBOARD_TRASH: &str = "confluence.whiteboard.trash";
pub const FOLDER_CREATE_ROUTE: &str = "confluence-folder-create";
pub const FOLDER_TRASH_ROUTE: &str = "confluence-folder-trash";
pub const WHITEBOARD_CREATE_ROUTE: &str = "confluence-whiteboard-create";
pub const WHITEBOARD_TRASH_ROUTE: &str = "confluence-whiteboard-trash";

/// Where new content goes. `root` sends no parent, and Confluence files the item under the space
/// homepage.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ContentPlacement {
    Root,
    Page {
        #[schemars(length(min = 1, max = 19), regex(pattern = "^[1-9][0-9]*$"))]
        id: String,
    },
    Folder {
        #[schemars(length(min = 1, max = 19), regex(pattern = "^[1-9][0-9]*$"))]
        id: String,
    },
}
impl ContentPlacement {
    pub fn parent_id(&self) -> Option<&str> {
        match self {
            Self::Root => None,
            Self::Page { id } | Self::Folder { id } => Some(id),
        }
    }
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContentCreateInput {
    pub space_id: String,
    #[schemars(length(min = 1, max = 255))]
    pub title: String,
    pub placement: ContentPlacement,
    /// Whiteboard only: a built-in template key such as `flow-chart` or `kanban-board`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = 64), regex(pattern = "^[0-9a-z][0-9a-z-]*$"))]
    pub template: Option<String>,
    /// Whiteboard only, with `template`: the template language such as `en-US`. Confluence
    /// otherwise uses the caller's locale.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 5, max = 5), regex(pattern = "^[a-z]{2}-[A-Z]{2}$"))]
    pub locale: Option<String>,
}

/// Template keys the pinned v2 specification enumerates for whiteboard create.
pub const WHITEBOARD_TEMPLATES: &[&str] = &[
    "2x2-prioritization",
    "4ls-retro",
    "annual-calendar",
    "brainwriting",
    "concept-map",
    "crazy-8s",
    "daily-sync",
    "disruptive-brainstorm",
    "dot-voting",
    "elevator-pitch",
    "flow-chart",
    "gap-analysis",
    "ice-breakers",
    "incident-postmortem",
    "journey-mapping-kit",
    "kanban-board",
    "lean-coffee",
    "network-of-teams",
    "org-chart",
    "pi-planning",
    "prioritization",
    "prioritization-experiment",
    "product-roadmap",
    "product-vision-board",
    "rice",
    "sailboat-retro",
    "service-blueprint",
    "simple-retrospective",
    "sprint-planning",
    "sticky-note-pack",
    "swimlanes",
    "team-formation-guide",
    "timeline",
    "timeline-workflow",
    "user-story-map",
    "workflow",
    "vision-board",
    "venn-diagram",
    "storyboard",
    "action-plan",
    "root-cause-analysis",
    "executive-summary",
    "stakeholder-mapping",
    "annual-calendar-2025-2026",
    "health-monitor",
    "okr-planning",
    "swot-analysis",
    "poker-planning",
    "fishbone-diagram",
    "risk-assessment",
    "bounded-context",
    "hopes-and-fears",
    "swimlane-vertical",
];
/// Template languages the pinned v2 specification enumerates.
pub const WHITEBOARD_LOCALES: &[&str] = &[
    "de-DE", "cs-CZ", "ko-KR", "fr-FR", "it-IT", "ja-JP", "nl-NL", "nb-NO", "da-DK", "sv-SE",
    "fi-FI", "ru-RU", "pl-PL", "tr-TR", "hu-HU", "en-GB", "en-US", "pt-BR", "zh-CN", "zh-TW",
    "es-ES",
];

impl ContentCreateInput {
    /// Templates exist only for whiteboards, and a locale only selects a template's language.
    pub(super) fn validate_for(&self, whiteboard: bool) -> Result<(), CraftError> {
        self.validate()?;
        match (&self.template, &self.locale) {
            (None, None) => Ok(()),
            _ if !whiteboard => Err(invalid("/template", "templates_are_whiteboard_only")),
            (None, Some(_)) => Err(invalid("/locale", "locale_requires_template")),
            (Some(template), _) if !WHITEBOARD_TEMPLATES.contains(&template.as_str()) => {
                Err(invalid("/template", "unknown_whiteboard_template"))
            }
            (_, Some(locale)) if !WHITEBOARD_LOCALES.contains(&locale.as_str()) => {
                Err(invalid("/locale", "unknown_template_locale"))
            }
            _ => Ok(()),
        }
    }
    fn validate(&self) -> Result<(), CraftError> {
        if !numeric(&self.space_id) || self.placement.parent_id().is_some_and(|id| !numeric(id)) {
            return Err(invalid("/input", "expected_numeric_space_and_parent_id"));
        }
        if !text(&self.title, usize::MAX)
            || self.title.chars().count() > 255
            || self.title.trim() != self.title
        {
            return Err(invalid("/title", "expected_bounded_nonempty_title"));
        }
        Ok(())
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContentRevision {
    pub version: u32,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContentTrashInput {
    pub space_id: String,
    pub id: String,
    pub expected: ContentRevision,
}
impl ContentTrashInput {
    pub(super) fn validate(&self) -> Result<(), CraftError> {
        if !numeric(&self.space_id) || !numeric(&self.id) {
            return Err(invalid("/input", "expected_numeric_space_and_content_id"));
        }
        if !(1..=i32::MAX as u32).contains(&self.expected.version) {
            return Err(invalid("/expected/version", "expected_reviewed_version"));
        }
        Ok(())
    }
}
pub fn content_kind(kind: PageWriteKind) -> Option<ContentKind> {
    use PageWriteKind as K;
    match kind {
        K::FolderCreate | K::FolderTrash => Some(ContentKind::Folder),
        K::WhiteboardCreate | K::WhiteboardTrash => Some(ContentKind::Whiteboard),
        _ => None,
    }
}

/// Checks the space, and the named parent page or folder, are current in that space and compiles
/// one POST. Root placement takes no parent evidence.
pub fn prepare_content_create(
    operation: &PreparedOperation,
    space: &NativeResponse,
    parent: Option<&NativeResponse>,
) -> Result<PreparedPageWrite, CraftError> {
    let kind = PageWriteKind::from_command(&operation.request().command)
        .filter(|k| {
            matches!(
                k,
                PageWriteKind::FolderCreate | PageWriteKind::WhiteboardCreate
            )
        })
        .ok_or_else(|| CraftError::field("/command", "expected_content_create"))?;
    validate_operation(operation, kind.command())?;
    let input: ContentCreateInput = input::parse(&operation.request().input)?;
    input.validate_for(kind == PageWriteKind::WhiteboardCreate)?;
    let s = current(space)?;
    if string(s, "id")? != input.space_id {
        return Err(malformed());
    }
    match (&input.placement, parent) {
        (ContentPlacement::Root, None) => {}
        (ContentPlacement::Page { id }, Some(p)) => {
            let p = current(p)?;
            if string(p, "id")? != id || string(p, "spaceId")? != input.space_id {
                return Err(malformed());
            }
        }
        (ContentPlacement::Folder { id }, Some(p)) => {
            let folder = observe(&p.body, ContentKind::Folder)?;
            if p.status != 200
                || folder.id != *id
                || folder.space_id != input.space_id
                || folder.status != "current"
            {
                return Err(malformed());
            }
        }
        _ => return Err(invalid("/placement", "parent_evidence_mismatch")),
    }
    let mut body = json!({"spaceId":input.space_id,"title":input.title});
    if let Some(id) = input.placement.parent_id() {
        body["parentId"] = json!(id);
    }
    if let Some(template) = &input.template {
        body["templateKey"] = json!(template);
    }
    if let Some(locale) = &input.locale {
        body["locale"] = json!(locale);
    }
    let call = NativeCall {
        route: kind.route().into(),
        path_parameters: BTreeMap::new(),
        query: BTreeMap::new(),
        repeated_query: BTreeMap::new(),
        body: Some(body),
    };
    registration::route(kind).validate_call(&call)?;
    Ok(PreparedPageWrite {
        kind,
        operation_digest: operation.digest().into(),
        site: operation.request().target.clone(),
        space_id: input.space_id,
        page_id: None,
        parent_id: input.placement.parent_id().map(Into::into),
        id: None,
        title: input.title,
        version: 1,
        call,
    })
}

/// Rechecks the reviewed version of a current folder or whiteboard in the space, then compiles one
/// DELETE, which moves it to the trash.
pub fn prepare_content_trash(
    operation: &PreparedOperation,
    item: &NativeResponse,
) -> Result<PreparedPageWrite, CraftError> {
    let kind = PageWriteKind::from_command(&operation.request().command)
        .filter(|k| {
            matches!(
                k,
                PageWriteKind::FolderTrash | PageWriteKind::WhiteboardTrash
            )
        })
        .ok_or_else(|| CraftError::field("/command", "expected_content_trash"))?;
    validate_operation(operation, kind.command())?;
    let input: ContentTrashInput = input::parse(&operation.request().input)?;
    let content = content_kind(kind).ok_or_else(malformed)?;
    if item.status != 200 {
        return Err(malformed());
    }
    let observed = observe(&item.body, content)?;
    if observed.id != input.id {
        return Err(malformed());
    }
    if observed.space_id != input.space_id
        || observed.status != "current"
        || observed.version != input.expected.version
    {
        return Err(CraftError::new(
            ErrorCode::VersionConflict,
            "The item changed since it was reviewed. Read it again before trashing it.",
        ));
    }
    // Native trash also trashes everything inside; only empty items are trashed, so nothing
    // unreviewed moves with it.
    if has_children(&item.body)? {
        return Err(CraftError::new(
            ErrorCode::VersionConflict,
            "The item is not empty. Trash or move its children first; trashing it would also trash them.",
        ));
    }
    let call = NativeCall {
        route: kind.route().into(),
        path_parameters: BTreeMap::from([("id".into(), input.id.clone())]),
        query: BTreeMap::new(),
        repeated_query: BTreeMap::new(),
        body: None,
    };
    registration::route(kind).validate_call(&call)?;
    Ok(PreparedPageWrite {
        kind,
        operation_digest: operation.digest().into(),
        site: operation.request().target.clone(),
        space_id: input.space_id,
        page_id: None,
        parent_id: None,
        id: Some(input.id),
        title: observed.title,
        version: observed.version,
        call,
    })
}

impl PreparedPageWrite {
    /// A created folder or whiteboard is confirmed by a current item of that type with a numeric
    /// ID in the space, the requested title and, when one was named, the requested parent.
    pub(super) fn interpret_content(
        &self,
        raw: &Value,
        mut result: PageWriteObservation,
    ) -> PageWriteObservation {
        let Some(kind) = content_kind(self.kind) else {
            return result;
        };
        if let Ok(item) = observe(raw, kind)
            && item.space_id == self.space_id
        {
            result.resources.push(ResourceRef {
                resource_type: kind.native().into(),
                id: item.id.clone(),
                ..self.site.clone()
            });
            let parent_ok = self.parent_id.is_none() || item.parent_id == self.parent_id;
            if item.status == "current"
                && item.title == self.title
                && raw.get("type").is_some_and(|t| t == kind.native())
                && parent_ok
            {
                result.state = SubmissionState::Confirmed;
                result.version = Some(item.version);
                return result;
            }
        }
        result.error = Some(super::result::unknown());
        result
    }
}
