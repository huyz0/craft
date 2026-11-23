//! One declarative entry per Trello read. Registration, routes, validation and execution all
//! derive from this table so a command cannot drift from its native route.

use craft_core::execution::{HttpMethod, NativeRoute, RouteSegment};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// One resource by id.
    Get,
    /// Trello returns the whole collection; craft slices it by offset.
    Sliced,
    /// Trello pages by `before` id and `limit` (actions).
    Before,
    /// Trello search pages by `cards_page`.
    SearchPage,
}

pub struct Entry {
    pub id: &'static str,
    pub kind: Kind,
    /// Path below the API root. `{id}` is the single path parameter.
    pub path: &'static [&'static str],
    /// Fixed query parameters sent on every call.
    pub query: &'static [(&'static str, &'static str)],
    pub description: &'static str,
}

const BOARD_FIELDS: &str = "name,desc,closed,url,dateLastActivity";
const LIST_FIELDS: &str = "name,closed,pos,idBoard";
const CARD_FIELDS: &str = "name,desc,closed,due,dueComplete,idList,idBoard,idLabels,idMembers,url,shortLink,dateLastActivity,pos,badges";

pub const ENTRIES: &[Entry] = &[
    Entry {
        id: "trello.board.list",
        kind: Kind::Sliced,
        path: &["members", "me", "boards"],
        query: &[("filter", "open"), ("fields", BOARD_FIELDS)],
        description: "List the open boards the credential can see.",
    },
    Entry {
        id: "trello.board.get",
        kind: Kind::Get,
        path: &["boards", "{id}"],
        query: &[("fields", BOARD_FIELDS)],
        description: "Read one board.",
    },
    Entry {
        id: "trello.list.list",
        kind: Kind::Sliced,
        path: &["boards", "{id}", "lists"],
        query: &[("filter", "open"), ("fields", LIST_FIELDS)],
        description: "List the open lists on a board.",
    },
    Entry {
        id: "trello.card.list",
        kind: Kind::Sliced,
        path: &["lists", "{id}", "cards"],
        query: &[("filter", "open"), ("fields", CARD_FIELDS)],
        description: "List the open cards in a list.",
    },
    Entry {
        id: "trello.card.board-list",
        kind: Kind::Sliced,
        path: &["boards", "{id}", "cards"],
        query: &[("filter", "open"), ("fields", CARD_FIELDS)],
        description: "List every open card on a board.",
    },
    Entry {
        id: "trello.card.get",
        kind: Kind::Get,
        path: &["cards", "{id}"],
        query: &[("fields", CARD_FIELDS)],
        description: "Read one card.",
    },
    Entry {
        id: "trello.card.search",
        kind: Kind::SearchPage,
        path: &["search"],
        query: &[
            ("modelTypes", "cards"),
            ("card_fields", CARD_FIELDS),
            ("partial", "false"),
        ],
        description: "Search cards by text, optionally within one board.",
    },
    Entry {
        id: "trello.comment.list",
        kind: Kind::Before,
        path: &["cards", "{id}", "actions"],
        query: &[("filter", "commentCard")],
        description: "List a card's comments, newest first.",
    },
    Entry {
        id: "trello.label.list",
        kind: Kind::Sliced,
        path: &["boards", "{id}", "labels"],
        query: &[("fields", "name,color,idBoard")],
        description: "List the labels defined on a board.",
    },
    Entry {
        id: "trello.checklist.list",
        kind: Kind::Sliced,
        path: &["cards", "{id}", "checklists"],
        query: &[("fields", "name,idCard,pos")],
        description: "List a card's checklists with their items.",
    },
    Entry {
        id: "trello.attachment.list",
        kind: Kind::Sliced,
        path: &["cards", "{id}", "attachments"],
        query: &[("fields", "name,url,bytes,date,mimeType,isUpload")],
        description: "List a card's attachments.",
    },
    Entry {
        id: "trello.member.list",
        kind: Kind::Sliced,
        path: &["boards", "{id}", "members"],
        query: &[("fields", "username,fullName")],
        description: "List the members of a board, for assignment.",
    },
];

pub fn commands() -> impl Iterator<Item = &'static str> {
    ENTRIES.iter().map(|entry| entry.id)
}

pub(crate) fn entry(command: &str) -> Option<&'static Entry> {
    ENTRIES.iter().find(|entry| entry.id == command)
}

pub fn kind_of(command: &str) -> Option<Kind> {
    entry(command).map(|entry| entry.kind)
}

pub fn is_get(command: &str) -> bool {
    kind_of(command) == Some(Kind::Get)
}

/// Route ids stay unique per command; the path comes straight from the entry.
pub(crate) fn route(entry: &Entry) -> NativeRoute {
    let mut query: Vec<String> = entry.query.iter().map(|(name, _)| (*name).into()).collect();
    let extra: &[&str] = match entry.kind {
        Kind::Get => &[],
        Kind::Sliced => &[],
        Kind::Before => &["limit", "before"],
        Kind::SearchPage => &["query", "idBoards", "cards_limit", "cards_page"],
    };
    query.extend(extra.iter().map(|name| (*name).to_owned()));
    NativeRoute {
        id: entry.id.replace('.', "-"),
        method: HttpMethod::Get,
        path: entry
            .path
            .iter()
            .map(|part| {
                if *part == "{id}" {
                    RouteSegment::Parameter("id".into())
                } else {
                    RouteSegment::Literal((*part).into())
                }
            })
            .collect(),
        query_parameters: query,
        repeated_query_parameters: Vec::new(),
        continuation_parameters: match entry.kind {
            Kind::Before => vec!["before".into()],
            Kind::SearchPage => vec!["cards_page".into()],
            _ => Vec::new(),
        },
        response_max_bytes: 2_097_152,
    }
}
