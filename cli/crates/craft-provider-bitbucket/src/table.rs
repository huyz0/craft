//! One declarative entry per Bitbucket read. Registration, routes, validation and execution all
//! derive from this table so a command cannot drift from its native route.

use craft_core::execution::{HttpMethod, NativeRoute, RouteSegment};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// One resource by its path identity.
    Get,
    /// A Bitbucket collection (`values`, `pagelen`, `next`) paged by its opaque `page` token.
    Paged,
}

/// Path parameters a command takes from its input. The workspace is never one of them: it comes
/// from the bound target.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Param {
    Repo,
    PullRequest,
    Pipeline,
}

impl Param {
    /// The route parameter name, which is also the input field that supplies it.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Repo => "repo",
            Self::PullRequest => "pull_request_id",
            Self::Pipeline => "pipeline_uuid",
        }
    }
}

/// An optional input narrowing a list, sent as a fixed native query parameter.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Filter {
    pub field: &'static str,
    pub query: &'static str,
}

pub struct Entry {
    pub id: &'static str,
    pub kind: Kind,
    /// Path below the API root. `{workspace}` comes from the target; other `{x}` entries are the
    /// `params` in order.
    pub path: &'static [&'static str],
    pub params: &'static [Param],
    /// Fixed query parameters sent on every call.
    pub query: &'static [(&'static str, &'static str)],
    /// Optional list filters.
    pub filters: &'static [Filter],
    pub description: &'static str,
}

pub const ENTRIES: &[Entry] = &[
    Entry {
        id: "bitbucket.repository.list",
        kind: Kind::Paged,
        path: &["repositories", "{workspace}"],
        params: &[],
        query: &[("sort", "-updated_on")],
        filters: &[],
        description: "List the repositories in the workspace, most recently updated first.",
    },
    Entry {
        id: "bitbucket.repository.get",
        kind: Kind::Get,
        path: &["repositories", "{workspace}", "{repo}"],
        params: &[Param::Repo],
        query: &[],
        filters: &[],
        description: "Read one repository.",
    },
    Entry {
        id: "bitbucket.branch.list",
        kind: Kind::Paged,
        path: &["repositories", "{workspace}", "{repo}", "refs", "branches"],
        params: &[Param::Repo],
        query: &[("sort", "-target.date")],
        filters: &[Filter {
            field: "name_contains",
            query: "q",
        }],
        description: "List a repository's branches, newest commit first; optionally by name.",
    },
    Entry {
        id: "bitbucket.commit.list",
        kind: Kind::Paged,
        path: &["repositories", "{workspace}", "{repo}", "commits"],
        params: &[Param::Repo],
        query: &[],
        filters: &[Filter {
            field: "branch",
            query: "include",
        }],
        description: "List a repository's commits, newest first; optionally those reachable from a branch.",
    },
    Entry {
        id: "bitbucket.pullrequest.list",
        kind: Kind::Paged,
        path: &["repositories", "{workspace}", "{repo}", "pullrequests"],
        params: &[Param::Repo],
        query: &[("sort", "-updated_on")],
        filters: &[Filter {
            field: "state",
            query: "state",
        }],
        description: "List a repository's pull requests (open unless a state is given).",
    },
    Entry {
        id: "bitbucket.pullrequest.get",
        kind: Kind::Get,
        path: &[
            "repositories",
            "{workspace}",
            "{repo}",
            "pullrequests",
            "{pull_request_id}",
        ],
        params: &[Param::Repo, Param::PullRequest],
        query: &[],
        filters: &[],
        description: "Read one pull request with its reviewers and participants.",
    },
    Entry {
        id: "bitbucket.pullrequest.diffstat",
        kind: Kind::Paged,
        path: &[
            "repositories",
            "{workspace}",
            "{repo}",
            "pullrequests",
            "{pull_request_id}",
            "diffstat",
        ],
        params: &[Param::Repo, Param::PullRequest],
        query: &[],
        filters: &[],
        description: "List the files a pull request changes with added and removed line counts.",
    },
    Entry {
        id: "bitbucket.comment.list",
        kind: Kind::Paged,
        path: &[
            "repositories",
            "{workspace}",
            "{repo}",
            "pullrequests",
            "{pull_request_id}",
            "comments",
        ],
        params: &[Param::Repo, Param::PullRequest],
        query: &[("sort", "created_on")],
        filters: &[],
        description: "List a pull request's comments, oldest first, including inline ones.",
    },
    Entry {
        id: "bitbucket.pipeline.list",
        kind: Kind::Paged,
        path: &["repositories", "{workspace}", "{repo}", "pipelines"],
        params: &[Param::Repo],
        query: &[("sort", "-created_on")],
        filters: &[],
        description: "List a repository's pipeline runs, newest first.",
    },
    Entry {
        id: "bitbucket.pipeline.get",
        kind: Kind::Get,
        path: &[
            "repositories",
            "{workspace}",
            "{repo}",
            "pipelines",
            "{pipeline_uuid}",
        ],
        params: &[Param::Repo, Param::Pipeline],
        query: &[],
        filters: &[],
        description: "Read one pipeline run.",
    },
    Entry {
        id: "bitbucket.step.list",
        kind: Kind::Paged,
        path: &[
            "repositories",
            "{workspace}",
            "{repo}",
            "pipelines",
            "{pipeline_uuid}",
            "steps",
        ],
        params: &[Param::Repo, Param::Pipeline],
        query: &[],
        filters: &[],
        description: "List a pipeline run's steps with their results.",
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

/// Route ids stay unique per command; the path comes straight from the entry. Paged routes also
/// declare `pagelen` and `page`, and every filter, so a validated next link can be followed.
pub(crate) fn route(entry: &Entry) -> NativeRoute {
    let mut query: Vec<String> = entry.query.iter().map(|(name, _)| (*name).into()).collect();
    for filter in entry.filters {
        query.push(filter.query.into());
    }
    if entry.kind == Kind::Paged {
        query.push("pagelen".into());
        query.push("page".into());
    }
    NativeRoute {
        id: entry.id.replace('.', "-"),
        method: HttpMethod::Get,
        path: entry
            .path
            .iter()
            .map(
                |part| match part.strip_prefix('{').and_then(|p| p.strip_suffix('}')) {
                    Some(name) => RouteSegment::Parameter(name.into()),
                    None => RouteSegment::Literal((*part).into()),
                },
            )
            .collect(),
        query_parameters: query,
        repeated_query_parameters: Vec::new(),
        continuation_parameters: if entry.kind == Kind::Paged {
            vec!["page".into()]
        } else {
            Vec::new()
        },
        response_max_bytes: 2_097_152,
    }
}
