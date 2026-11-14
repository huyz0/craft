//! Live read conformance against a real site. Opt-in only:
//!
//! ```text
//! $env:CRAFT_LIVE = '1'
//! cargo test -p craft-live-conformance --offline --test live_reads -- --ignored --nocapture
//! ```
//!
//! Identifiers are discovered from the site at run time, so nothing tenant
//! specific is compiled in. Every read runs through the provider's registered
//! handler; a failure means the provider rejected or misparsed a real response
//! (or the site lacks the data a case needs, which the summary says).

use std::sync::Arc;

use craft_core::registry::{ProviderModule, Registry};
use craft_live_conformance::{Live, Outcome, capture_dir};
use craft_provider_confluence as confluence;
use craft_provider_jira as jira;
use serde_json::{Value, json};

type TestResult = Result<(), Box<dyn std::error::Error>>;

struct FixedClock;
impl craft_core::storage::Clock for FixedClock {
    fn unix_millis(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }
    fn monotonic_millis(&self) -> u64 {
        self.unix_millis()
    }
}

fn registry(
    register: fn(&mut Registry) -> Result<(), craft_core::error::CraftError>,
) -> Result<Registry, String> {
    let mut registry = Registry::default();
    register(&mut registry).map_err(|e| e.message)?;
    Ok(registry)
}

fn module(module: &dyn ProviderModule) -> Result<Registry, String> {
    let mut registry = Registry::default();
    module.register(&mut registry).map_err(|e| e.message)?;
    Ok(registry)
}

fn str_at<'a>(value: &'a Value, pointer: &str) -> Option<&'a str> {
    value.pointer(pointer).and_then(Value::as_str)
}

/// Real identifiers from the site; any may be missing on a sparse site.
#[derive(Default, Debug)]
struct Discovered {
    project_id: Option<String>,
    project_key: Option<String>,
    issue_id: Option<String>,
    issue_key: Option<String>,
    issue_type_id: Option<String>,
    comment_id: Option<String>,
    attachment_id: Option<String>,
    link_id: Option<String>,
    account_id: Option<String>,
    space_id: Option<String>,
    page_id: Option<String>,
    footer_comment_id: Option<String>,
    reply_parent_id: Option<String>,
}

fn discover(live: &Live) -> Discovered {
    let mut d = Discovered::default();
    if let Ok(me) = live.get("/rest/api/3/myself") {
        d.account_id = str_at(&me.body, "/accountId").map(String::from);
    }
    if let Ok(projects) = live.get("/rest/api/3/project/search?maxResults=10") {
        d.project_id = str_at(&projects.body, "/values/0/id").map(String::from);
        d.project_key = str_at(&projects.body, "/values/0/key").map(String::from);
    }
    if let Some(key) = &d.project_key {
        // Prefer an issue that has comments, attachments or links so those reads have data.
        let jql = format!("project = {key} ORDER BY updated DESC");
        let path = format!(
            "/rest/api/3/search/jql?maxResults=50&fields=issuetype,comment,attachment,issuelinks&jql={}",
            url_encode(&jql)
        );
        if let Ok(search) = live.get(&path) {
            let issues = search.body["issues"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let score = |issue: &Value| {
                let f = &issue["fields"];
                [
                    f["comment"]["comments"]
                        .as_array()
                        .is_some_and(|a| !a.is_empty()),
                    f["attachment"].as_array().is_some_and(|a| !a.is_empty()),
                    f["issuelinks"].as_array().is_some_and(|a| !a.is_empty()),
                ]
                .iter()
                .filter(|x| **x)
                .count()
            };
            if let Some(issue) = issues.iter().max_by_key(|i| score(i)) {
                d.issue_id = str_at(issue, "/id").map(String::from);
                d.issue_key = str_at(issue, "/key").map(String::from);
                d.issue_type_id = str_at(issue, "/fields/issuetype/id").map(String::from);
            }
            for issue in &issues {
                let f = &issue["fields"];
                if d.comment_id.is_none()
                    && let Some(id) = str_at(f, "/comment/comments/0/id")
                {
                    d.comment_id = Some(id.into());
                }
                if d.attachment_id.is_none() {
                    d.attachment_id = str_at(f, "/attachment/0/id").map(String::from);
                }
                if d.link_id.is_none() {
                    d.link_id = str_at(f, "/issuelinks/0/id").map(String::from);
                }
            }
        }
    }
    if let Ok(spaces) = live.get("/wiki/api/v2/spaces?limit=10") {
        d.space_id = str_at(&spaces.body, "/results/0/id").map(String::from);
    }
    if let Ok(pages) = live.get("/wiki/api/v2/pages?limit=25") {
        let pages = pages.body["results"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        d.page_id = str_at(pages.first().unwrap_or(&Value::Null), "/id").map(String::from);
        for page in &pages {
            let Some(id) = str_at(page, "/id") else {
                continue;
            };
            if let Ok(comments) =
                live.get(&format!("/wiki/api/v2/pages/{id}/footer-comments?limit=25"))
                && let Some(comment) = str_at(&comments.body, "/results/0/id")
            {
                d.page_id = Some(id.into());
                d.footer_comment_id = Some(comment.into());
                for c in comments.body["results"].as_array().into_iter().flatten() {
                    let Some(cid) = str_at(c, "/id") else {
                        continue;
                    };
                    if let Ok(children) = live.get(&format!(
                        "/wiki/api/v2/footer-comments/{cid}/children?limit=1"
                    )) && str_at(&children.body, "/results/0/id").is_some()
                    {
                        d.reply_parent_id = Some(cid.into());
                        break;
                    }
                }
                break;
            }
        }
    }
    d
}

fn url_encode(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

enum Case {
    Ran(Outcome),
    Skipped(&'static str, &'static str),
}

#[test]
#[ignore = "live tenant access; opt in with CRAFT_LIVE=1"]
fn live_read_conformance() -> TestResult {
    let live = Live::connect()?;
    let d = discover(&live);
    eprintln!(
        "discovered: project={} issue={} comment={} attachment={} link={} space={} page={} footer_comment={} reply_parent={}",
        d.project_key.is_some(),
        d.issue_id.is_some(),
        d.comment_id.is_some(),
        d.attachment_id.is_some(),
        d.link_id.is_some(),
        d.space_id.is_some(),
        d.page_id.is_some(),
        d.footer_comment_id.is_some(),
        d.reply_parent_id.is_some()
    );

    let jira_site = live.site_target("jira");
    let cf_site = live.site_target("confluence");
    let projects = module(&jira::projects::JiraProjects)?;
    let project_context = module(&jira::project_context::JiraProjectContext)?;
    let fields = module(&jira::fields::JiraFields)?;
    let issues = module(&jira::issue_reads::JiraIssues)?;
    let issue_context = module(&jira::issue_context::JiraIssueContext)?;
    let permissions = module(&jira::permissions::JiraPermissions)?;
    let metadata = module(&jira::create_metadata::JiraMetadata::new(Arc::new(
        FixedClock,
    )))?;
    let reconciliation = module(&jira::reconciliation::JiraReconciliation)?;
    let comments = registry(jira::comments::register)?;
    let attachments = registry(jira::attachments::register)?;
    let links = registry(jira::issue_links::reads::register)?;
    let cf_context = module(&confluence::ConfluenceContext)?;

    let mut cases: Vec<Case> = Vec::new();
    let mut run = |registry: &Registry, command: &str, target, input: Value| {
        cases.push(Case::Ran(live.run_in(registry, command, target, input)));
    };

    run(
        &projects,
        "jira.project.list",
        jira_site.clone(),
        json!({"page_size": 50}),
    );
    run(&fields, "jira.field.list", jira_site.clone(), json!({}));
    run(&fields, "jira.field.search", jira_site.clone(), json!({}));
    run(
        &attachments,
        "jira.attachment.settings",
        jira_site.clone(),
        json!({}),
    );
    run(&links, "jira.link.types", jira_site.clone(), json!({}));

    let mut skipped: Vec<(&'static str, &'static str)> = Vec::new();
    match (&d.project_key, &d.project_id) {
        (Some(key), Some(id)) => {
            let project = live.target("jira", "project", key);
            run(
                &project_context,
                "jira.project.get",
                project.clone(),
                json!({}),
            );
            run(
                &project_context,
                "jira.project.statuses",
                project.clone(),
                json!({}),
            );
            if let Some(account) = &d.account_id {
                run(
                    &metadata,
                    "jira.metadata.assignee",
                    project.clone(),
                    json!({"account_id": account}),
                );
            }
            if let Some(type_id) = &d.issue_type_id {
                run(
                    &metadata,
                    "jira.metadata.create",
                    live.target("jira", "project", id),
                    json!({"issue_type_id": type_id, "page_size": 50}),
                );
            }
            run(
                &issues,
                "jira.issue.search",
                jira_site.clone(),
                json!({"jql": format!("project = {key} ORDER BY key"), "page_size": 50, "reconcile_issue_ids": []}),
            );
            if let Some(issue) = &d.issue_id {
                run(
                    &reconciliation,
                    "jira.operation.inspect",
                    live.target("jira", "project", id),
                    json!({"issue_ids": [issue]}),
                );
            }
        }
        _ => {
            skipped.push(("jira.project.get", "no project on site"));
            skipped.push(("jira.project.statuses", "no project on site"));
        }
    }
    match (&d.issue_id, &d.issue_key) {
        (Some(id), Some(key)) => {
            run(
                &issues,
                "jira.issue.get",
                jira_site.clone(),
                json!({"issue": id}),
            );
            let issue = live.target("jira", "issue", id);
            run(
                &issue_context,
                "jira.metadata.edit",
                issue.clone(),
                json!({"issue": id}),
            );
            run(
                &issue_context,
                "jira.transition.list",
                issue.clone(),
                json!({"issue": id}),
            );
            run(
                &permissions,
                "jira.permission.check",
                jira_site.clone(),
                json!({"comment_id": null, "issue": key, "permissions": ["BROWSE_PROJECTS", "EDIT_ISSUES"], "project": null}),
            );
            run(
                &comments,
                "jira.comment.list",
                jira_site.clone(),
                json!({"issue_id": id, "page_size": 50}),
            );
            run(
                &attachments,
                "jira.attachment.list",
                jira_site.clone(),
                json!({"issue_id": id}),
            );
            run(
                &links,
                "jira.link.list",
                jira_site.clone(),
                json!({"issue_id": id}),
            );
            match &d.comment_id {
                Some(comment) => run(
                    &comments,
                    "jira.comment.get",
                    jira_site.clone(),
                    json!({"issue_id": id, "comment_id": comment}),
                ),
                None => skipped.push(("jira.comment.get", "no comment found on recent issues")),
            }
        }
        _ => skipped.push(("jira.issue.get and issue-scoped reads", "no issue on site")),
    }
    match &d.attachment_id {
        Some(a) => run(
            &attachments,
            "jira.attachment.get",
            jira_site.clone(),
            json!({"id": a}),
        ),
        None => skipped.push((
            "jira.attachment.get",
            "no attachment found on recent issues",
        )),
    }
    match &d.link_id {
        Some(l) => run(
            &links,
            "jira.link.get",
            jira_site.clone(),
            json!({"link_id": l}),
        ),
        None => skipped.push(("jira.link.get", "no issue link found on recent issues")),
    }

    run(
        &cf_context,
        "confluence.space.list",
        cf_site.clone(),
        json!({"page_size": 25}),
    );
    run(
        &cf_context,
        "confluence.page.list",
        cf_site.clone(),
        json!({"page_size": 25}),
    );
    run(
        &cf_context,
        "confluence.page.search",
        cf_site.clone(),
        json!({"cql": "type = page", "page_size": 25}),
    );
    match &d.space_id {
        Some(space) => run(
            &cf_context,
            "confluence.space.get",
            cf_site.clone(),
            json!({"id": space}),
        ),
        None => skipped.push(("confluence.space.get", "no space on site")),
    }
    match &d.page_id {
        Some(page) => {
            run(
                &cf_context,
                "confluence.page.get",
                cf_site.clone(),
                json!({"id": page}),
            );
            run(
                &cf_context,
                "confluence.comment.list",
                cf_site.clone(),
                json!({"page_id": page, "page_size": 25}),
            );
            match &d.footer_comment_id {
                Some(c) => run(
                    &cf_context,
                    "confluence.comment.get",
                    cf_site.clone(),
                    json!({"id": c, "page_id": page}),
                ),
                None => skipped.push(("confluence.comment.get", "no footer comment found")),
            }
            match &d.reply_parent_id {
                Some(parent) => run(
                    &cf_context,
                    "confluence.comment.replies",
                    cf_site.clone(),
                    json!({"page_id": page, "page_size": 25, "parent_comment_id": parent}),
                ),
                None => skipped.push((
                    "confluence.comment.replies",
                    "no footer comment with replies found",
                )),
            }
        }
        None => skipped.push(("confluence.page.get and comment reads", "no page on site")),
    }
    for (command, reason) in skipped {
        cases.push(Case::Skipped(command, reason));
    }

    let mut report = Vec::new();
    let (mut pass, mut fail, mut skip) = (0, 0, 0);
    for case in &cases {
        match case {
            Case::Ran(outcome) => {
                if outcome.passed() {
                    pass += 1
                } else {
                    fail += 1
                }
                report.push(outcome.summary());
            }
            Case::Skipped(command, reason) => {
                skip += 1;
                report.push(format!("{command} => SKIP ({reason})"));
            }
        }
    }
    report.push(format!("\n{pass} passed, {fail} failed, {skip} skipped"));
    let text = report.join("\n");
    eprintln!("{text}");
    std::fs::create_dir_all(capture_dir())?;
    std::fs::write(capture_dir().join("summary.txt"), &text)?;
    if fail > 0 {
        return Err(format!("{fail} live read case(s) failed; see target/live-captures").into());
    }
    Ok(())
}
