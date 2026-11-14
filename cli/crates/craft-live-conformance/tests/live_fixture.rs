//! Adds a `craft-test` attachment to an existing Confluence page with the account token, as
//! fixture data for CLI attachment reads and trash (the CLI has no Confluence upload yet).
//! Opt-in; the page ID comes from `CRAFT_LIVE_PAGE`. Cleanup purges the page and its files.

use craft_core::execution::HttpMethod;
use craft_live_conformance::{Body, Live};

#[test]
#[ignore = "changes the live test site; opt in with CRAFT_LIVE=1 and CRAFT_LIVE_PAGE"]
fn live_confluence_attachment_fixture() -> Result<(), Box<dyn std::error::Error>> {
    let page = std::env::var("CRAFT_LIVE_PAGE")?;
    if page.is_empty() || !page.bytes().all(|b| b.is_ascii_digit()) {
        return Err("CRAFT_LIVE_PAGE must be a numeric page id".into());
    }
    let mut live = Live::connect()?;
    live.allow_writes(true);
    let boundary = "craft-live-boundary-cf01".to_owned();
    let mut bytes = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"craft-test.txt\"\r\nContent-Type: text/plain\r\n\r\n"
    )
    .into_bytes();
    bytes.extend_from_slice(b"craft-test attachment\n");
    bytes.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let out = live.raw(
        HttpMethod::Post,
        &format!("/wiki/rest/api/content/{page}/child/attachment"),
        Body::Multipart { boundary, bytes },
    )?;
    let id = out.body["results"][0]["id"].as_str().unwrap_or_default();
    println!("attachment {} -> {id}", out.status);
    Ok(())
}
