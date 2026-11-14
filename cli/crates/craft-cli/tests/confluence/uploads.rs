use super::*;
use craft_cli::confluence_attachment_upload::command::UploadArgs;
use craft_cli::confluence_attachment_upload::{PreparedUpload, UploadEnvironment};
use craft_cli::profiles::InvocationOptions;
use craft_core::input::content_digest;
use craft_runtime::confluence_write_policy::{
    ConfluenceWriteOperation as Op, ConfluenceWritePolicyUpdate,
};
use craft_runtime::journal::NativeJournal;
use std::fs;

type UploadResult = Result<(), Box<dyn std::error::Error>>;
const UPLOAD_SCOPES: &str = "read:confluence-user read:page:confluence read:attachment:confluence read:content-details:confluence write:attachment:confluence";

fn space() -> ResourceRef {
    ResourceRef {
        resource_type: "space".into(),
        id: "100".into(),
        ..target()
    }
}
fn allow(setup: &Setup) -> Result<(), CraftError> {
    setup.store.set_confluence_write_policy(
        "factory",
        ConfluenceWritePolicyUpdate {
            target: space(),
            expected_actor: "actor-one".into(),
            operations: vec![Op::AttachmentUpload, Op::AttachmentReplace],
            enabled: true,
            expected_revision: None,
        },
        true,
        &control()?,
        |_| Ok(()),
    )?;
    Ok(())
}
fn args(
    setup: &Setup,
    operation: &str,
    bytes: &[u8],
    replace: bool,
) -> Result<UploadArgs, Box<dyn std::error::Error>> {
    let input = setup.root.join("input");
    fs::create_dir_all(&input)?;
    fs::write(input.join("source.bin"), bytes)?;
    Ok(UploadArgs {
        operation_id: operation.into(),
        space_id: Some("100".into()),
        page_id: "101".into(),
        attachment_id: replace.then(|| "att301".into()),
        expected: replace.then(|| {
            craft_provider_confluence::attachment_upload::AttachmentRevision {
                version: 2,
                file_id: "file-1".into(),
            }
        }),
        artifact_name: format!("{operation}.bin"),
        file_root: Some(input.to_str().ok_or("path")?.into()),
        file: Some("source.bin".into()),
        from_artifact: false,
        expected_digest: None,
        filename: "design.bin".into(),
        media_type: "application/octet-stream".into(),
        byte_limit: 1_000_000,
        cloud_id: Some(CLOUD.into()),
        apply: true,
    })
}
fn options(setup: &Setup) -> InvocationOptions<'_> {
    InvocationOptions {
        state_root: Some(&setup.root),
        profile: Some("factory"),
        request_id: "upload-original",
        max_bytes: 8_388_608,
    }
}
fn current_attachment(version: u64, file: &str) -> Value {
    json!({"id":"att301","pageId":"101","status":"current","title":"design.bin","mediaType":"application/octet-stream","fileSize":3,"fileId":file,"version":{"number":version}})
}
fn file_part(request: &tls_fixture::Observed) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let body = &request.body;
    let marker = b"name=\"file\"";
    let start = body
        .windows(marker.len())
        .position(|v| v == marker)
        .ok_or("file part")?;
    let offset = body[start..]
        .windows(4)
        .position(|v| v == b"\r\n\r\n")
        .ok_or("file headers")?
        + start
        + 4;
    let boundary = request
        .headers
        .get("content-type")
        .and_then(|v| v.strip_prefix("multipart/form-data; boundary="))
        .ok_or("boundary")?;
    let suffix = format!("\r\n--{boundary}--\r\n");
    Ok(body[offset..body.len() - suffix.len()].to_vec())
}
fn run(
    setup: &Setup,
    input: UploadArgs,
    replies: Vec<tls_fixture::Reply>,
) -> Result<
    (
        craft_cli::dispatch::CommandResult,
        Vec<tls_fixture::Observed>,
    ),
    Box<dyn std::error::Error>,
> {
    let product = TlsServer::start(Reply::Sequence(replies))?;
    let issuer = issuer(UPLOAD_SCOPES)?;
    let fixture = setup.fixture(&issuer, &product, Arc::new(ManualClock::new(1000)))?;
    let locked = fixture.lock(&setup.grant, &control()?)?;
    fixture.client().provision_grant(
        &locked,
        fixture.service_credentials()?,
        &[target().tenant_ref()],
        Some("actor-one"),
    )?;
    let journal = NativeJournal::open(&setup.store, &control()?)?;
    let result = PreparedUpload::prepare(input, &[], &options(setup), &setup.store, &control()?)?
        .execute(
            UploadEnvironment {
                store: &setup.store,
                client: fixture.client(),
                locked: &locked,
                journal: &journal,
                tls: TlsPolicy::AdditionalCaBundle(product.root()),
                control: control()?,
            },
            || Ok(()),
        );
    drop(locked);
    Ok((result, product.finish()?))
}

#[test]
fn page_upload_refuses_existing_filename_and_sends_one_exact_multipart_snapshot() -> UploadResult {
    let bytes = b"abc".to_vec();
    for mode in ["upload", "existing_name"] {
        let setup = Setup::new()?;
        allow(&setup)?;
        let names = if mode == "existing_name" {
            json!({"results":[current_attachment(1, "file-1")],"_links":{}})
        } else {
            json!({"results":[],"_links":{}})
        };
        let mut replies = vec![
            identity(),
            identity(),
            reply(page(101))?,
            reply(names.clone())?,
        ];
        if mode == "upload" {
            replies.extend([
                reply(names.clone())?,
                reply(page(101))?,
                reply(json!({"results":[{"id":"att302","type":"attachment","status":"current","title":"design.bin","version":{"number":1},"extensions":{"mediaType":"application/octet-stream","fileSize":3}}]}))?,
            ]);
        }
        let (result, observed) = run(
            &setup,
            args(&setup, &format!("cf-{mode}"), &bytes, false)?,
            replies,
        )?;
        let data = result.envelope.data.clone().ok_or("data")?;
        let posts: Vec<_> = observed.iter().filter(|r| r.method == "POST").collect();
        if mode == "existing_name" {
            assert_eq!(
                result.envelope.error.ok_or("err")?.code,
                ErrorCode::VersionConflict
            );
            assert!(posts.is_empty());
            assert_eq!(data["receipt"]["request_state"], "not_sent");
            continue;
        }
        assert!(result.envelope.ok, "{:?}", result.envelope.error);
        assert_eq!(data["receipt"]["request_state"], "confirmed");
        assert_eq!(data["receipt"]["confirmed_resources"][0]["id"], "att302");
        assert_eq!(data["attachment_id"], "att302");
        assert_eq!(data["desired_fields_verified"], true);
        assert_eq!(data["remote_digest_verified"], false);
        assert_eq!(data["snapshot"]["digest"], content_digest(&bytes));
        assert_eq!(posts.len(), 1);
        assert!(
            posts[0]
                .target
                .ends_with("/wiki/rest/api/content/101/child/attachment"),
            "{}",
            posts[0].target
        );
        assert_eq!(
            posts[0]
                .headers
                .get("x-atlassian-token")
                .map(String::as_str),
            Some("no-check")
        );
        assert!(
            String::from_utf8_lossy(&posts[0].body).contains("name=\"minorEdit\"\r\n\r\ntrue\r\n")
        );
        assert_eq!(file_part(posts[0])?, bytes);
        let mut registry = craft_core::registry::Registry::default();
        craft_cli::confluence_attachment_upload::command::register(&mut registry)?;
        assert!(
            jsonschema::validator_for(
                &registry
                    .descriptor("confluence.attachment.upload")?
                    .response_schema
            )?
            .is_valid(&json!(result.envelope))
        );
        setup.no_secrets()?;
    }
    Ok(())
}

#[test]
fn replace_is_bound_to_the_reviewed_attachment_id_version_and_file() -> UploadResult {
    let bytes = b"xyz".to_vec();
    for mode in ["replace", "changed", "lost"] {
        let setup = Setup::new()?;
        allow(&setup)?;
        let observed_attachment = if mode == "changed" {
            current_attachment(3, "file-2")
        } else {
            current_attachment(2, "file-1")
        };
        let mut replies = vec![
            identity(),
            identity(),
            reply(page(101))?,
            reply(observed_attachment.clone())?,
        ];
        match mode {
            "replace" => replies.extend([
                reply(observed_attachment.clone())?,
                reply(page(101))?,
                reply(json!({"id":"att301","type":"attachment","status":"current","title":"design.bin","version":{"number":3},"extensions":{"mediaType":"application/octet-stream","fileSize":3}}))?,
            ]),
            "lost" => replies.extend([
                reply(observed_attachment.clone())?,
                reply(page(101))?,
                Reply::Disconnect,
            ]),
            _ => {}
        }
        let (result, observed) = run(
            &setup,
            args(&setup, &format!("cf-{mode}"), &bytes, true)?,
            replies,
        )?;
        let data = result.envelope.data.clone().ok_or("data")?;
        let posts: Vec<_> = observed.iter().filter(|r| r.method == "POST").collect();
        match mode {
            "changed" => {
                assert_eq!(
                    result.envelope.error.ok_or("err")?.code,
                    ErrorCode::VersionConflict
                );
                assert!(posts.is_empty());
            }
            "lost" => {
                assert_eq!(data["receipt"]["request_state"], "sent_unknown");
                assert_eq!(data["reconciliation_required"], true);
                assert_eq!(posts.len(), 1);
            }
            _ => {
                assert!(result.envelope.ok, "{:?}", result.envelope.error);
                assert_eq!(data["receipt"]["confirmed_resources"][0]["id"], "att301");
                assert_eq!(data["version"], 3);
                assert!(
                    posts[0]
                        .target
                        .ends_with("/wiki/rest/api/content/101/child/attachment/att301/data"),
                    "{}",
                    posts[0].target
                );
                assert_eq!(file_part(posts[0])?, bytes);
                assert_eq!(result.envelope.command, "confluence.attachment.replace");
            }
        }
        setup.no_secrets()?;
    }
    Ok(())
}
