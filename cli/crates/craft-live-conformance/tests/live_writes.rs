//! Live write lifecycle against the test site. Opt-in only, and it changes the site:
//!
//! ```text
//! $env:CRAFT_LIVE = '1'
//! cargo test -p craft-live-conformance --offline --test live_writes -- --ignored --nocapture
//! ```
//!
//! Every item it creates is titled with the `craft-test` prefix and is removed at
//! the end, whether or not earlier steps passed. It never edits or deletes data it
//! did not create in this run. Provider writes run the same sequence as the hosts:
//! the provider's bracketing reads and freeze through a session bound to the read
//! descriptor, one send of the frozen call through a session bound to the write
//! descriptor, then the provider's interpretation of the real response. Fixture
//! data that no public command manages (the issues themselves, an attachment, a
//! Confluence footer comment and reply) is created and removed with raw requests.
//! The five reads the read run skips for lack of data run here against that data.

use std::sync::Arc;

use craft_core::contracts::{GrantRef, ResourceRef};
use craft_core::error::CraftError;
use craft_core::execution::{HttpMethod, NativeResponse, OperationContext, PreparedOperation};
use craft_core::registry::{ProviderModule, Registry};
use craft_core::storage::SubmissionState;
use craft_live_conformance::{Body, Live, capture_dir};
use craft_provider_confluence as confluence;
use craft_provider_jira as jira;
use jira::create_metadata::MetadataPrincipal;
use serde_json::{Value, json};

type TestResult = Result<(), Box<dyn std::error::Error>>;
type Step = Result<String, String>;

struct Clock;
impl craft_core::storage::Clock for Clock {
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

fn err(e: CraftError) -> String {
    format!("{:?}: {} {:?}", e.code, e.message, e.details)
}

fn str_at(value: &Value, pointer: &str) -> Option<String> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(String::from)
}

fn registry_of(modules: &[&dyn ProviderModule]) -> Result<Registry, String> {
    let mut registry = Registry::default();
    for module in modules {
        module.register(&mut registry).map_err(err)?;
    }
    Ok(registry)
}

fn registry_fn(register: fn(&mut Registry) -> Result<(), CraftError>) -> Result<Registry, String> {
    let mut registry = Registry::default();
    register(&mut registry).map_err(err)?;
    Ok(registry)
}

/// Mirrors the issue-mutation host registry: issue context reads plus the
/// mutation and action descriptors.
fn issue_registry() -> Result<Registry, String> {
    let mut registry = Registry::default();
    registry
        .register_provider(&jira::issue_context::JiraIssueContext)
        .map_err(err)?;
    jira::issue_context::mutation::JiraIssueMutations
        .register(&mut registry)
        .map_err(err)?;
    jira::issue_context::mutation::actions::JiraIssueActions
        .register(&mut registry)
        .map_err(err)?;
    Ok(registry)
}

fn prepare(
    registry: &Registry,
    command: &str,
    target: ResourceRef,
    input: Value,
) -> Result<PreparedOperation, String> {
    registry
        .prepare(craft_core::contracts::CanonicalRequest {
            command: command.into(),
            target,
            input,
            selection: craft_core::contracts::Selection::default(),
            preconditions: None,
        })
        .map_err(err)
}

fn state(name: SubmissionState) -> &'static str {
    match name {
        SubmissionState::Confirmed => "confirmed",
        SubmissionState::Failed => "failed",
        SubmissionState::Unknown => "unknown",
        SubmissionState::Prepared => "prepared",
        SubmissionState::SendStarted => "send-started",
    }
}

fn adf(text: &str) -> Value {
    json!({"type":"doc","version":1,"content":[{"type":"paragraph","content":[{"type":"text","text":text}]}]})
}

struct Run<'a> {
    live: &'a Live,
    principal: MetadataPrincipal,
    /// Comment and link writes are bound to the issue's project.
    project_id: String,
    report: Vec<String>,
    failures: usize,
    created_issues: Vec<String>,
    deleted_issues: Vec<String>,
    attachments: Vec<String>,
    pages: Vec<String>,
}

impl Run<'_> {
    fn record(&mut self, name: &str, step: Step) -> Option<String> {
        match step {
            Ok(detail) => {
                self.report.push(format!("{name} => PASS {detail}"));
                Some(detail)
            }
            Err(error) => {
                self.failures += 1;
                self.report.push(format!("{name} => FAIL {error}"));
                None
            }
        }
    }

    /// Current issue state in the exact shape the mutation inputs expect.
    fn expected(&self, issue: &str) -> Result<Value, String> {
        let raw = self.live.get(&format!(
            "/rest/api/3/issue/{issue}?fields=project,issuetype,status,updated,assignee"
        ))?;
        if raw.status != 200 {
            return Err(format!("issue read returned {}", raw.status));
        }
        let b = &raw.body;
        Ok(json!({
            "id": str_at(b, "/id"),
            "key": str_at(b, "/key"),
            "project_id": str_at(b, "/fields/project/id"),
            "issue_type_id": str_at(b, "/fields/issuetype/id"),
            "status_id": str_at(b, "/fields/status/id"),
            "updated": str_at(b, "/fields/updated"),
        }))
    }

    fn create_issue(&mut self, project_id: &str, type_id: &str, summary: &str) -> Step {
        let body = json!({"fields":{"project":{"id":project_id},"issuetype":{"id":type_id},"summary":summary}});
        let out = self
            .live
            .raw(HttpMethod::Post, "/rest/api/3/issue", Body::Json(&body))?;
        let id = str_at(&out.body, "/id").ok_or(format!("issue create returned {}", out.status))?;
        self.created_issues.push(id.clone());
        Ok(id)
    }

    fn comment_write(&self, registry: &Registry, command: &str, input: Value) -> Step {
        let target = self.live.target("jira", "project", &self.project_id);
        let read_command = jira::comments::mutation::context_command(command).map_err(err)?;
        let read = prepare(registry, read_command, target.clone(), input.clone())?;
        let write = prepare(registry, command, target, input)?;
        let mut rctx = self.live.session(registry, read_command).map_err(err)?;
        let frozen = jira::comments::mutation::prepare(
            &read,
            &write,
            &mut rctx,
            self.principal.clone(),
            Arc::new(Clock),
        )
        .map_err(err)?;
        let mut wctx = self.live.session(registry, command).map_err(err)?;
        let response = wctx.invoke(frozen.call()).map_err(err)?;
        let observed = jira::comments::mutation::interpret(&write, &response).map_err(err)?;
        if let Some(error) = observed.error {
            return Err(format!(
                "{} after HTTP {}: {}",
                state(observed.state),
                response.status,
                err(error)
            ));
        }
        if observed.state != SubmissionState::Confirmed {
            return Err(format!(
                "state {} after HTTP {}",
                state(observed.state),
                response.status
            ));
        }
        Ok(observed
            .resources
            .iter()
            .find(|r| r.resource_type == "comment")
            .map(|r| r.id.clone())
            .unwrap_or_default())
    }

    fn comment_revision(
        &self,
        registry: &Registry,
        issue: &str,
        comment: &str,
    ) -> Result<String, String> {
        let op = prepare(
            registry,
            jira::comments::GET,
            self.live.site_target("jira"),
            json!({"issue_id": issue, "comment_id": comment}),
        )?;
        let mut ctx = self
            .live
            .session(registry, jira::comments::GET)
            .map_err(err)?;
        let page = jira::comments::read_page(&op, &mut ctx, None).map_err(err)?;
        page.items
            .first()
            .and_then(|i| str_at(i, "/revision"))
            .ok_or_else(|| "comment read returned no revision".into())
    }

    fn issue_action(&self, registry: &Registry, command: &str, issue: &str, input: Value) -> Step {
        use jira::issue_context::mutation::actions;
        let target = self.live.target("jira", "issue", issue);
        let read_command = actions::metadata_command(command).map_err(err)?;
        let read = prepare(registry, read_command, target.clone(), input.clone())?;
        let write = prepare(registry, command, target.clone(), input)?;
        let mut rctx = self.live.session(registry, read_command).map_err(err)?;
        let frozen = actions::prepare_action(&read, &write, &mut rctx, &self.principal, &Clock)
            .map_err(err)?;
        self.send_issue_mutation(registry, command, &target, frozen.call())
    }

    fn issue_edit(&self, registry: &Registry, issue: &str, input: Value) -> Step {
        use jira::issue_context::{EDIT, FieldPlan, read_context};
        let command = "jira.issue.update";
        let target = self.live.target("jira", "issue", issue);
        let read = prepare(registry, EDIT, target.clone(), json!({"issue": issue}))?;
        let write = prepare(registry, command, target.clone(), input)?;
        let mut rctx = self.live.session(registry, EDIT).map_err(err)?;
        let plan = FieldPlan::compile(EDIT, &[]).map_err(err)?;
        let metadata =
            read_context(&read, &mut rctx, &plan, None, &self.principal, &Clock).map_err(err)?;
        let frozen = metadata
            .prepare_mutation(&read, &write, &mut rctx, &self.principal, &Clock)
            .map_err(err)?;
        self.send_issue_mutation(registry, command, &target, frozen.call())
    }

    fn send_issue_mutation(
        &self,
        registry: &Registry,
        command: &str,
        target: &ResourceRef,
        call: &craft_core::execution::NativeCall,
    ) -> Step {
        let mut wctx = self.live.session(registry, command).map_err(err)?;
        let response = wctx.invoke(call).map_err(err)?;
        let observed = jira::issue_context::mutation::result::interpret(command, target, &response);
        if let Some(error) = observed.error {
            return Err(format!(
                "{} after HTTP {}: {}",
                state(observed.state),
                response.status,
                err(error)
            ));
        }
        if observed.state != SubmissionState::Confirmed {
            return Err(format!(
                "state {} after HTTP {}",
                state(observed.state),
                response.status
            ));
        }
        Ok(format!("HTTP {}", response.status))
    }

    fn link_create(&self, registry: &Registry, input: Value) -> Step {
        use jira::issue_links::write as links;
        let target = self.live.target("jira", "project", &self.project_id);
        let read = prepare(registry, links::CONTEXT, target.clone(), input.clone())?;
        let write = prepare(registry, links::CREATE, target.clone(), input.clone())?;
        let mut rctx = self.live.session(registry, links::CONTEXT).map_err(err)?;
        let frozen = links::prepare_link(
            &read,
            &write,
            &mut rctx,
            self.principal.clone(),
            Arc::new(Clock),
        )
        .map_err(err)?;
        let mut wctx = self.live.session(registry, links::CREATE).map_err(err)?;
        let response = wctx.invoke(frozen.call()).map_err(err)?;
        let parsed: links::LinkInput = serde_json::from_value(input).map_err(|e| e.to_string())?;
        let observed = links::interpret(&target, &parsed, &response);
        if let Some(error) = observed.error {
            return Err(format!(
                "{} after HTTP {}: {}",
                state(observed.state),
                response.status,
                err(error)
            ));
        }
        if observed.state != SubmissionState::Confirmed {
            return Err(format!(
                "state {} after HTTP {}",
                state(observed.state),
                response.status
            ));
        }
        Ok(format!("HTTP {}", response.status))
    }

    fn link_delete(&self, registry: &Registry, input: Value) -> Step {
        use jira::issue_links::write as links;
        let target = self.live.target("jira", "project", &self.project_id);
        let read = prepare(
            registry,
            links::DELETE_CONTEXT,
            target.clone(),
            input.clone(),
        )?;
        let write = prepare(registry, links::DELETE, target.clone(), input.clone())?;
        let mut rctx = self
            .live
            .session(registry, links::DELETE_CONTEXT)
            .map_err(err)?;
        let frozen = links::prepare_delete(
            &read,
            &write,
            &mut rctx,
            self.principal.clone(),
            Arc::new(Clock),
        )
        .map_err(err)?;
        let mut wctx = self.live.session(registry, links::DELETE).map_err(err)?;
        let response = wctx.invoke(frozen.call()).map_err(err)?;
        let parsed: links::LinkDeleteInput =
            serde_json::from_value(input).map_err(|e| e.to_string())?;
        let observed = links::interpret_delete(&target, &parsed, &response);
        if let Some(error) = observed.error {
            return Err(format!(
                "{} after HTTP {}: {}",
                state(observed.state),
                response.status,
                err(error)
            ));
        }
        if observed.state != SubmissionState::Confirmed {
            return Err(format!(
                "state {} after HTTP {}",
                state(observed.state),
                response.status
            ));
        }
        Ok(format!("HTTP {}", response.status))
    }

    /// Runs one read through the provider's read entry point.
    fn read(
        &self,
        registry: &Registry,
        command: &str,
        input: Value,
        read: impl FnOnce(&PreparedOperation, &mut dyn OperationContext) -> Result<usize, CraftError>,
    ) -> Step {
        let target = self.live.site_target(if command.starts_with("jira") {
            "jira"
        } else {
            "confluence"
        });
        let op = prepare(registry, command, target, input)?;
        let mut ctx = self.live.session(registry, command).map_err(err)?;
        let count = read(&op, &mut ctx).map_err(err)?;
        if count == 0 {
            return Err("provider returned no item for data known to exist".into());
        }
        Ok(format!("{count} item(s)"))
    }

    fn cleanup(&mut self) {
        for id in std::mem::take(&mut self.attachments) {
            let r = self.live.raw(
                HttpMethod::Delete,
                &format!("/rest/api/3/attachment/{id}"),
                Body::None,
            );
            self.report.push(format!(
                "cleanup attachment => {}",
                r.map(|e| e.status.to_string()).unwrap_or_else(|e| e)
            ));
        }
        for id in std::mem::take(&mut self.pages) {
            let trash = self.live.raw(
                HttpMethod::Delete,
                &format!("/wiki/api/v2/pages/{id}"),
                Body::None,
            );
            let purge = self.live.raw(
                HttpMethod::Delete,
                &format!("/wiki/api/v2/pages/{id}?purge=true"),
                Body::None,
            );
            self.report.push(format!(
                "cleanup page => trash {} purge {}",
                trash.map(|e| e.status.to_string()).unwrap_or_else(|e| e),
                purge.map(|e| e.status.to_string()).unwrap_or_else(|e| e)
            ));
        }
        for id in std::mem::take(&mut self.created_issues) {
            if self.deleted_issues.contains(&id) {
                continue;
            }
            let r = self.live.raw(
                HttpMethod::Delete,
                &format!("/rest/api/3/issue/{id}?deleteSubtasks=false"),
                Body::None,
            );
            self.report.push(format!(
                "cleanup issue => {}",
                r.map(|e| e.status.to_string()).unwrap_or_else(|e| e)
            ));
        }
    }
}

#[test]
#[ignore = "changes the live test site; opt in with CRAFT_LIVE=1"]
fn live_write_lifecycle() -> TestResult {
    let mut live = Live::connect()?;
    live.allow_writes(true);
    let live = live;
    let me = live.get("/rest/api/3/myself")?;
    let account = str_at(&me.body, "/accountId").ok_or("no account id")?;
    let projects = live.get("/rest/api/3/project/search?maxResults=10")?;
    let project_id = str_at(&projects.body, "/values/0/id").ok_or("no project")?;
    let types = live.get(&format!(
        "/rest/api/3/issue/createmeta/{project_id}/issuetypes"
    ))?;
    let type_id = types.body["issueTypes"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|t| t["subtask"] == json!(false) && t["name"] == json!("Task"))
        .or_else(|| {
            types.body["issueTypes"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|t| t["subtask"] == json!(false))
        })
        .and_then(|t| str_at(t, "/id"))
        .ok_or("no non-subtask issue type")?;
    let link_types = live.get("/rest/api/3/issueLinkType")?;
    let link_type = link_types.body["issueLinkTypes"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|t| t["name"] == json!("Blocks"))
        .or_else(|| link_types.body["issueLinkTypes"].as_array().and_then(|a| a.first()))
        .map(|t| json!({"id": t["id"], "name": t["name"], "inward": t["inward"], "outward": t["outward"]}))
        .ok_or("no link type")?;
    let spaces = live.get("/wiki/api/v2/spaces?limit=10")?;
    let space_id = str_at(&spaces.body, "/results/0/id").ok_or("no space")?;

    let stamp = craft_core::storage::Clock::unix_millis(&Clock);
    let label = format!("craft-test live {stamp}");
    let mut run = Run {
        live: &live,
        principal: MetadataPrincipal {
            grant: GrantRef {
                id: "gr_00000000000000000000000000000000".into(),
            },
            actor: account.clone(),
        },
        project_id: project_id.clone(),
        report: Vec::new(),
        failures: 0,
        created_issues: Vec::new(),
        deleted_issues: Vec::new(),
        attachments: Vec::new(),
        pages: Vec::new(),
    };

    let outcome = (|| -> Result<(), String> {
        let comments = registry_fn(jira::comments::mutation::register)?;
        let comment_reads = registry_fn(jira::comments::register)?;
        let link_writes = registry_fn(jira::issue_links::write::register)?;
        let link_reads = registry_fn(jira::issue_links::reads::register)?;
        let attachment_reads = registry_fn(jira::attachments::register)?;
        let issues = issue_registry()?;
        let pages = registry_of(&[&confluence::pages::ConfluencePages])?;
        let cf_reads = registry_of(&[&confluence::ConfluenceContext])?;

        let a = run.create_issue(&project_id, &type_id, &format!("{label} A"));
        let a = run
            .record("fixture: create issue A", a)
            .ok_or("cannot continue without issue A")?;
        let b = run.create_issue(&project_id, &type_id, &format!("{label} B"));
        let b = run
            .record("fixture: create issue B", b)
            .ok_or("cannot continue without issue B")?;

        // Comment create, read, update, delete.
        let created = run.comment_write(&comments, jira::comments::mutation::CREATE,
            json!({"issue_id": a, "body": adf(&format!("{label} comment")), "visibility": {"mode": "project"}}));
        if let Some(comment) = run.record("jira.comment.create", created) {
            let read = run.read(
                &comment_reads,
                jira::comments::GET,
                json!({"issue_id": a, "comment_id": comment}),
                |op, ctx| jira::comments::read_page(op, ctx, None).map(|p| p.items.len()),
            );
            run.record("jira.comment.get", read);
            let updated = run.comment_revision(&comment_reads, &a, &comment).and_then(|revision| {
                run.comment_write(&comments, jira::comments::mutation::UPDATE, json!({"issue_id": a, "comment_id": comment,
                    "expected_revision": revision, "ownership": "own", "body": adf(&format!("{label} comment edited"))}))
            });
            run.record("jira.comment.update", updated);
            let deleted = run
                .comment_revision(&comment_reads, &a, &comment)
                .and_then(|revision| {
                    run.comment_write(
                        &comments,
                        jira::comments::mutation::DELETE,
                        json!({"issue_id": a, "comment_id": comment,
                    "expected_revision": revision, "ownership": "own"}),
                    )
                });
            run.record("jira.comment.delete", deleted);
        }

        // Link create, read, delete.
        let link_input = json!({"outward_issue_id": a, "inward_issue_id": b, "inward_project_id": project_id, "link_type": link_type});
        let linked = run.link_create(&link_writes, link_input.clone());
        if run.record("jira.link.create", linked).is_some() {
            let link_id = live
                .get(&format!("/rest/api/3/issue/{a}?fields=issuelinks"))
                .ok()
                .and_then(|r| str_at(&r.body, "/fields/issuelinks/0/id"));
            match link_id {
                Some(link_id) => {
                    let read = run.read(
                        &link_reads,
                        "jira.link.get",
                        json!({"link_id": link_id}),
                        |op, ctx| {
                            jira::issue_links::reads::read_page(op, ctx, None)
                                .map(|p| p.items.len())
                        },
                    );
                    run.record("jira.link.get", read);
                    let deleted = run.link_delete(
                        &link_writes,
                        json!({"link_id": link_id, "expected": link_input}),
                    );
                    run.record("jira.link.delete", deleted);
                }
                None => {
                    run.record(
                        "jira.link.get",
                        Err("created link not visible on issue".into()),
                    );
                }
            }
        }

        // Attachment fixture and read.
        let uploaded = live.upload_attachment(&a, "craft-test.txt", b"craft-test attachment\n");
        let attachment = match uploaded {
            Ok(e) if e.status == 200 => str_at(&e.body, "/0/id"),
            Ok(e) => {
                run.record(
                    "fixture: upload attachment",
                    Err(format!("HTTP {}", e.status)),
                );
                None
            }
            Err(e) => {
                run.record("fixture: upload attachment", Err(e));
                None
            }
        };
        if let Some(id) = attachment {
            run.attachments.push(id.clone());
            run.record("fixture: upload attachment", Ok("HTTP 200".into()));
            let read = run.read(
                &attachment_reads,
                jira::attachments::GET,
                json!({"id": id}),
                |op, ctx| jira::attachments::read_page(op, ctx, None).map(|p| p.items.len()),
            );
            run.record("jira.attachment.get", read);
        }

        // Issue edit, assign, unassign.
        let edited = run.expected(&a).and_then(|expected| run.issue_edit(&issues, &a,
            json!({"issue": a, "expected": expected, "changes": {"summary": format!("{label} A edited")}})));
        run.record("jira.issue.update", edited);
        let assigned = run.expected(&a).and_then(|expected| run.issue_action(&issues, "jira.issue.assign", &a,
            json!({"issue": a, "expected": expected, "expected_assignee": {"kind": "unassigned"}, "account_id": account})));
        run.record("jira.issue.assign", assigned);
        let unassigned = run.expected(&a).and_then(|expected| run.issue_action(&issues, "jira.issue.unassign", &a,
            json!({"issue": a, "expected": expected, "expected_assignee": {"kind": "account", "account_id": account}})));
        run.record("jira.issue.unassign", unassigned);

        // Confluence page create, then comment fixtures and reads on that page.
        let created_page = (|| -> Step {
            let space = live.get(&format!("/wiki/api/v2/spaces/{space_id}"))?;
            let op = prepare(
                &pages,
                "confluence.page.create",
                live.site_target("confluence"),
                json!({
                "space_id": space_id, "title": format!("{label} page"), "placement": {"kind": "root"},
                "body": {"representation": "storage", "value": format!("<p>{label} page body</p>")}}),
            )?;
            let space_response = NativeResponse {
                status: space.status,
                body: space.body,
                request_id: None,
                retry_after: None,
                link_headers: None,
            };
            let prepared =
                confluence::pages::prepare_create(&op, &space_response, None).map_err(err)?;
            let mut wctx = live
                .session(&pages, "confluence.page.create")
                .map_err(err)?;
            let response = wctx.invoke(prepared.call()).map_err(err)?;
            let observed = prepared.interpret(&response);
            if let Some(error) = observed.error {
                return Err(format!(
                    "{} after HTTP {}: {}",
                    state(observed.state),
                    response.status,
                    err(error)
                ));
            }
            observed
                .resources
                .iter()
                .find(|r| r.resource_type == "page")
                .map(|r| r.id.clone())
                .ok_or_else(|| format!("no page resource after HTTP {}", response.status))
        })();
        if let Some(page) = run.record("confluence.page.create", created_page) {
            run.pages.push(page.clone());
            let body = json!({"pageId": page, "body": {"representation": "storage", "value": format!("<p>{label} comment</p>")}});
            let comment = live
                .raw(
                    HttpMethod::Post,
                    "/wiki/api/v2/footer-comments",
                    Body::Json(&body),
                )
                .ok()
                .and_then(|e| str_at(&e.body, "/id"));
            run.record(
                "fixture: footer comment",
                comment.clone().ok_or_else(|| "create failed".to_string()),
            );
            if let Some(comment) = comment {
                let reply = json!({"parentCommentId": comment, "body": {"representation": "storage", "value": format!("<p>{label} reply</p>")}});
                let replied = live
                    .raw(
                        HttpMethod::Post,
                        "/wiki/api/v2/footer-comments",
                        Body::Json(&reply),
                    )
                    .ok()
                    .and_then(|e| str_at(&e.body, "/id"));
                run.record(
                    "fixture: footer reply",
                    replied.ok_or_else(|| "create failed".to_string()),
                );
                let read = run.read(
                    &cf_reads,
                    "confluence.comment.get",
                    json!({"id": comment, "page_id": page}),
                    |op, ctx| {
                        confluence::comments::read_get(op, ctx).map(|v| usize::from(v.is_object()))
                    },
                );
                run.record("confluence.comment.get", read);
                let read = run.read(
                    &cf_reads,
                    "confluence.comment.replies",
                    json!({"page_id": page, "page_size": 25, "parent_comment_id": comment}),
                    |op, ctx| {
                        confluence::comments::paging::read_page(op, ctx, None)
                            .map(|p| p.items.len())
                    },
                );
                run.record("confluence.comment.replies", read);
            }
        }

        // Provider deletion of both issues; raw cleanup covers any that fail.
        for issue in [b.clone(), a.clone()] {
            let deleted =
                run.expected(&issue).and_then(|expected| {
                    run.issue_action(&issues, "jira.issue.delete", &issue,
                json!({"issue": issue, "expected": expected, "subtasks": {"mode": "refuse"}}))
                });
            if run.record("jira.issue.delete", deleted).is_some() {
                run.deleted_issues.push(issue);
            }
        }
        Ok(())
    })();
    if let Err(error) = outcome {
        run.failures += 1;
        run.report.push(format!("aborted: {error}"));
    }
    run.cleanup();
    let failures = run.failures;
    let text = run.report.join("\n");
    eprintln!("{text}\n\n{failures} failure(s)");
    std::fs::create_dir_all(capture_dir())?;
    std::fs::write(capture_dir().join("writes-summary.txt"), &text)?;
    if failures > 0 {
        return Err(format!("{failures} live write step(s) failed").into());
    }
    Ok(())
}
