//! Removes leftover `craft-test` Jira issues and Confluence pages created by live
//! runs, using the account API token. Only items whose title starts with
//! `craft-test` are touched. Opt-in like the other live tests.

use craft_core::execution::HttpMethod;
use craft_live_conformance::{Body, Live};

#[test]
#[ignore = "changes the live test site; opt in with CRAFT_LIVE=1"]
fn live_cleanup_craft_test_items() -> Result<(), Box<dyn std::error::Error>> {
    let mut live = Live::connect()?;
    live.allow_writes(true);
    let jql: String = url::form_urlencoded::byte_serialize(b"summary ~ \"craft-test\"").collect();
    let issues = live.get(&format!(
        "/rest/api/3/search/jql?maxResults=50&fields=summary&jql={jql}"
    ))?;
    for issue in issues.body["issues"].as_array().into_iter().flatten() {
        let summary = issue["fields"]["summary"].as_str().unwrap_or_default();
        let Some(id) = issue["id"].as_str() else {
            continue;
        };
        if !summary.starts_with("craft-test") {
            continue;
        }
        let out = live.raw(
            HttpMethod::Delete,
            &format!("/rest/api/3/issue/{id}?deleteSubtasks=false"),
            Body::None,
        )?;
        eprintln!("issue {id} -> {}", out.status);
    }
    let cql: String =
        url::form_urlencoded::byte_serialize(b"title ~ \"craft-test\" and type = page").collect();
    let pages = live.get(&format!("/wiki/rest/api/search?limit=50&cql={cql}"))?;
    for result in pages.body["results"].as_array().into_iter().flatten() {
        let title = result["content"]["title"].as_str().unwrap_or_default();
        let Some(id) = result["content"]["id"].as_str() else {
            continue;
        };
        if !title.starts_with("craft-test") {
            continue;
        }
        let trash = live.raw(
            HttpMethod::Delete,
            &format!("/wiki/api/v2/pages/{id}"),
            Body::None,
        )?;
        let purge = live.raw(
            HttpMethod::Delete,
            &format!("/wiki/api/v2/pages/{id}?purge=true"),
            Body::None,
        )?;
        eprintln!("page {id} -> {} {}", trash.status, purge.status);
    }
    // Pages the CLI already moved to the trash no longer appear in CQL search; purge them too.
    let trashed = live.get("/wiki/api/v2/pages?status=trashed&limit=250")?;
    for page in trashed.body["results"].as_array().into_iter().flatten() {
        let title = page["title"].as_str().unwrap_or_default();
        let Some(id) = page["id"].as_str() else {
            continue;
        };
        if !title.starts_with("craft-test") {
            continue;
        }
        let purge = live.raw(
            HttpMethod::Delete,
            &format!("/wiki/api/v2/pages/{id}?purge=true"),
            Body::None,
        )?;
        eprintln!("trashed page {id} -> {}", purge.status);
    }
    // Spaces created by `tools/live_host.py space`; deletion runs as a native long task.
    let spaces = live.get("/wiki/api/v2/spaces?limit=250")?;
    for space in spaces.body["results"].as_array().into_iter().flatten() {
        let key = space["key"].as_str().unwrap_or_default();
        if !key.starts_with("CRAFTTEST") || !key.bytes().all(|b| b.is_ascii_alphanumeric()) {
            continue;
        }
        let out = live.raw(
            HttpMethod::Delete,
            &format!("/wiki/rest/api/space/{key}"),
            Body::None,
        )?;
        eprintln!("space {key} -> {}", out.status);
    }
    Ok(())
}
