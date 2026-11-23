use std::path::PathBuf;

use clap::Args;
use craft_factory_evidence::{
    bitbucket_change::{
        MAX_APPROVAL_BYTES, MAX_PULL_REQUEST_BYTES, approved_pull_request_endpoint, collect,
    },
    bitbucket_review::{
        self, MAX_ACTIVITY_BYTES, MAX_COMMENT_BYTES, MAX_POLICY_BYTES, REVIEW_REFERENCE,
    },
    release_trace::{Change, read_evidence},
    sha256,
};
use serde_json::{Value, json};

use crate::{
    Result,
    bitbucket_read::ReadClient,
    evidence_capture::{create_directory, now, require_new_directory, write_new},
    reject,
};

#[derive(Args)]
pub struct Options {
    /// Approved change record bound to the reviewed head and merge result.
    approval: PathBuf,
    /// SHA-256 digest of the exact change record from the approving channel.
    #[arg(long)]
    approval_digest: String,
    /// New directory for the original evidence and manifest change section.
    #[arg(long)]
    destination: PathBuf,
}

#[derive(Args)]
pub struct ReviewOptions {
    /// Approved scope binding the exact human reviewer, PR comment, intent, and merge result.
    policy: PathBuf,
    /// SHA-256 digest of the exact policy from the approving channel.
    #[arg(long)]
    policy_digest: String,
    /// New directory for the original evidence and manifest change section.
    #[arg(long)]
    destination: PathBuf,
}

pub fn run(options: Options) -> Result<Value> {
    capture(
        options,
        |url| read_client()?.get(url, MAX_PULL_REQUEST_BYTES),
        now,
    )
}

pub fn run_review(options: ReviewOptions) -> Result<Value> {
    capture_review(
        options,
        |policy| {
            let client = read_client()?;
            let snapshot = client.get(&policy.endpoint(), MAX_PULL_REQUEST_BYTES)?;
            let comment = client.get(&policy.comment_endpoint(), MAX_COMMENT_BYTES)?;
            let activity = policy
                .collect_activity(|url| {
                    client
                        .get(url, MAX_ACTIVITY_BYTES)
                        .map_err(|error| error.to_string().into())
                })
                .map_err(|error| error.to_string())?;
            Ok((snapshot, comment, activity))
        },
        now,
    )
}

fn read_client() -> Result<ReadClient> {
    ReadClient::from_environment(
        "CRAFT_FACTORY_BITBUCKET_EVIDENCE_TOKEN",
        "CRAFT_FACTORY_BITBUCKET_EVIDENCE_EMAIL",
    )
}

fn capture(
    options: Options,
    fetch: impl FnOnce(&str) -> Result<Vec<u8>>,
    time: impl FnOnce() -> Result<u64>,
) -> Result<Value> {
    require_new_directory(&options.destination)?;
    let approval =
        read_evidence(&options.approval, MAX_APPROVAL_BYTES).map_err(|error| error.to_string())?;
    let endpoint = approved_pull_request_endpoint(&approval, &options.approval_digest)
        .map_err(|error| error.to_string())?;
    let snapshot = fetch(&endpoint)?;
    let observed_at = time()?;
    let change = collect(&snapshot, &approval, &options.approval_digest, observed_at)
        .map_err(|error| error.to_string())?;
    if change.approval_reference.starts_with(REVIEW_REFERENCE) {
        return reject(
            "use the original Bitbucket review policy to collect native review evidence",
        );
    }
    write_archive(options.destination, &snapshot, &approval, &change, None)
}

fn capture_review(
    options: ReviewOptions,
    fetch: impl FnOnce(&bitbucket_review::Policy) -> Result<(Vec<u8>, Vec<u8>, Vec<Vec<u8>>)>,
    time: impl FnOnce() -> Result<u64>,
) -> Result<Value> {
    require_new_directory(&options.destination)?;
    let raw =
        read_evidence(&options.policy, MAX_POLICY_BYTES).map_err(|error| error.to_string())?;
    let policy = bitbucket_review::Policy::parse(&raw, &options.policy_digest)
        .map_err(|error| error.to_string())?;
    let (snapshot, comment, activity) = fetch(&policy)?;
    let (approval, change) = policy
        .verify(&snapshot, &comment, &activity, time()?)
        .map_err(|error| error.to_string())?;
    write_archive(
        options.destination,
        &snapshot,
        &approval,
        &change,
        Some(ReviewArchive {
            policy: &raw,
            comment: &comment,
            activity: &activity,
        }),
    )
}

struct ReviewArchive<'a> {
    policy: &'a [u8],
    comment: &'a [u8],
    activity: &'a [Vec<u8>],
}

fn write_archive(
    destination: PathBuf,
    snapshot: &[u8],
    approval: &[u8],
    change: &Change,
    review: Option<ReviewArchive<'_>>,
) -> Result<Value> {
    let observed_at = change.observation.observed_at;
    let mut source = json!({
        "pull_request": {"path": "pull-request.json", "sha256": sha256(snapshot)},
        "approval": {"path": "approval.json", "sha256": sha256(approval)},
        "observed_at": observed_at,
    });
    if let Some(ReviewArchive {
        policy,
        comment,
        activity,
    }) = &review
    {
        let mut review_source = json!({
            "policy": {"path": "review-policy.json", "sha256": sha256(policy)},
            "comment": {"path": "review-comment.json", "sha256": sha256(comment)},
        });
        if !activity.is_empty() {
            review_source["activity"] = json!(
                activity
                    .iter()
                    .enumerate()
                    .map(|(index, page)| json!({
                        "path": format!("merge-activity-{index}.json"), "sha256": sha256(page),
                    }))
                    .collect::<Vec<_>>()
            );
        }
        source["review"] = review_source;
    }
    let source = serde_json::to_vec_pretty(&source)?;
    create_directory(&destination)?;
    write_new(&destination.join("pull-request.json"), snapshot)?;
    write_new(&destination.join("approval.json"), approval)?;
    if let Some(ReviewArchive {
        policy,
        comment,
        activity,
    }) = review
    {
        write_new(&destination.join("review-policy.json"), policy)?;
        write_new(&destination.join("review-comment.json"), comment)?;
        for (index, page) in activity.iter().enumerate() {
            write_new(
                &destination.join(format!("merge-activity-{index}.json")),
                page,
            )?;
        }
    }
    let source_path = destination.join("change-source.json");
    write_new(&source_path, &source)?;
    Ok(json!({
        "schema_version": 1, "source": source_path, "source_digest": sha256(&source),
        "pull_request": change.pull_request, "result_commit": change.result_commit,
        "observed_at": observed_at,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn captures_only_the_pinned_merged_change_and_preserves_original_bytes() {
        let mut nonce = [0; 8];
        getrandom::getrandom(&mut nonce).unwrap();
        let root =
            std::env::temp_dir().join(format!("factory-change-{:x}", u64::from_ne_bytes(nonce)));
        create_directory(&root).unwrap();
        let approval_path = root.join("approved.json");
        let destination = root.join("archive");
        let uuid = "12345678-4321-1234-abcd-fedcba987654";
        let head = "a".repeat(40);
        let result = "b".repeat(40);
        let approval = serde_json::to_vec(&json!({
            "schema_version": 1, "workspace": "team", "repository": "service",
            "repository_uuid": uuid, "destination_branch": "main", "pull_request_id": 2,
            "approved_source_commit": head, "result_commit": result, "work_item": "FIXTURE-2",
            "intent_reference": "fixture:intent", "intent_revision": "r3",
            "approval_reference": "fixture:approval", "approved_at": 1795237990_u64,
            "merge_reference": "fixture:merge", "merged_at": 1795238000_u64,
        }))
        .unwrap();
        write_new(&approval_path, &approval).unwrap();
        let digest = sha256(&approval);
        let options = |pin: String| Options {
            approval: approval_path.clone(),
            approval_digest: pin,
            destination: destination.clone(),
        };
        let observed_at = 1795238030;
        let clock = || Ok(observed_at);
        let bad_pin = capture(
            options(sha256(b"different")),
            |_| panic!("invalid pin must not fetch"),
            clock,
        );
        assert!(bad_pin.unwrap_err().to_string().contains("approved digest"));
        let mut response = json!({
            "type": "pullrequest", "id": 2, "state": "MERGED", "source": {"commit": {"hash": head}},
            "destination": {"branch": {"name": "main"}, "repository": {"full_name": "team/service", "uuid": uuid}},
            "merge_commit": {"hash": "c".repeat(40)},
            "created_on": "2026-11-21T05:11:00Z", "updated_on": "2026-11-21T05:13:25Z",
        });
        let fetch = |url: &str, body: &Value| {
            assert_eq!(
                url,
                "https://api.bitbucket.org/2.0/repositories/team/service/pullrequests/2"
            );
            Ok(serde_json::to_vec_pretty(body).unwrap())
        };
        let wrong_merge = capture(options(digest.clone()), |url| fetch(url, &response), clock);
        assert!(
            wrong_merge
                .unwrap_err()
                .to_string()
                .contains("exact merge result")
        );
        assert!(!destination.exists());
        response["merge_commit"]["hash"] = json!(result);
        let receipt = capture(options(digest.clone()), |url| fetch(url, &response), clock).unwrap();
        let snapshot = fs::read(destination.join("pull-request.json")).unwrap();
        assert_eq!(snapshot, serde_json::to_vec_pretty(&response).unwrap());
        assert_eq!(
            fs::read(destination.join("approval.json")).unwrap(),
            approval
        );
        let bytes = fs::read(destination.join("change-source.json")).unwrap();
        assert_eq!(receipt["source_digest"], sha256(&bytes));
        assert_eq!(receipt["result_commit"], result);
        let source: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            source,
            json!({
                "pull_request": {"path": "pull-request.json", "sha256": sha256(&snapshot)},
                "approval": {"path": "approval.json", "sha256": digest},
                "observed_at": observed_at,
            })
        );
        let repeated = capture(
            options(digest),
            |_| panic!("existing archive must not fetch"),
            clock,
        );
        assert!(repeated.unwrap_err().to_string().contains("already exists"));

        let reviewer = "12345678-4321-1234-abcd-fedcba987656";
        let mut policy: Value = serde_json::from_slice(&approval).unwrap();
        let fields = policy.as_object_mut().unwrap();
        fields.remove("approval_reference");
        fields.remove("approved_at");
        fields.insert("approver_uuid".into(), json!(reviewer));
        fields.insert("approval_comment_id".into(), json!(51));
        let policy = serde_json::to_vec(&policy).unwrap();
        let policy_path = root.join("review-policy.json");
        write_new(&policy_path, &policy).unwrap();
        let review_destination = root.join("review-archive");
        let options = |policy_digest| ReviewOptions {
            policy: policy_path.clone(),
            policy_digest,
            destination: review_destination.clone(),
        };
        let rejected = capture_review(
            options(sha256(b"wrong")),
            |_| panic!("an unpinned policy must not fetch"),
            clock,
        );
        assert!(
            rejected
                .unwrap_err()
                .to_string()
                .contains("approved digest")
        );
        assert!(!review_destination.exists());
        response["author"] = json!({"uuid": uuid});
        let decision = json!({"decision":"approve","source_commit":head,"work_item":"FIXTURE-2",
                              "intent_reference":"fixture:intent","intent_revision":"r3"});
        let comment = serde_json::to_vec(&json!({
            "type":"pullrequest_comment","id":51,"deleted":false,"pullrequest":{"id":2},
            "user":{"uuid":reviewer},"content":{"raw":format!("Craft release approval v1\n{decision}")},
            "created_on":"2026-11-21T05:13:10Z","updated_on":"2026-11-21T05:13:10.020Z",
        })).unwrap();
        let captured = capture_review(
            options(sha256(&policy)),
            |scope| {
                let snapshot = fetch(&scope.endpoint(), &response)?;
                assert_eq!(
                    scope.comment_endpoint(),
                    format!("{}/comments/51", scope.endpoint())
                );
                let activity = scope
                    .collect_activity(|_| panic!("external merge policy must not request activity"))
                    .map_err(|error| error.to_string())?;
                Ok((snapshot, comment.clone(), activity))
            },
            clock,
        )
        .unwrap();
        let bytes = fs::read(review_destination.join("change-source.json")).unwrap();
        assert_eq!(captured["source_digest"], sha256(&bytes));
        let source: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(source["review"]["comment"]["sha256"], sha256(&comment));
        assert_eq!(
            fs::read(review_destination.join("review-comment.json")).unwrap(),
            comment
        );
        assert_eq!(source["review"]["policy"]["sha256"], sha256(&policy));
        assert_eq!(
            fs::read(review_destination.join("review-policy.json")).unwrap(),
            policy
        );
        let mut native_policy: Value = serde_json::from_slice(&policy).unwrap();
        let fields = native_policy.as_object_mut().unwrap();
        fields.remove("merged_at");
        fields.remove("merge_reference");
        let raw = serde_json::to_vec(&native_policy).unwrap();
        fs::write(&policy_path, &raw).unwrap();
        let activity_destination = root.join("review-with-activity");
        let activity = serde_json::to_vec(&json!({"values":[{
            "update":{"state":"MERGED","date":"2026-11-21T05:13:20Z",
                "source":response["source"],"destination":response["destination"]},
            "pull_request":{"id":2}
        }]}))
        .unwrap();
        capture_review(
            ReviewOptions {
                policy: policy_path,
                policy_digest: sha256(&raw),
                destination: activity_destination.clone(),
            },
            |scope| {
                let snapshot = fetch(&scope.endpoint(), &response)?;
                let pages = scope
                    .collect_activity(|url| {
                        assert_eq!(url, format!("{}/activity", scope.endpoint()));
                        Ok(activity.clone())
                    })
                    .map_err(|error| error.to_string())?;
                Ok((snapshot, comment, pages))
            },
            clock,
        )
        .unwrap();
        assert_eq!(
            fs::read(activity_destination.join("merge-activity-0.json")).unwrap(),
            activity
        );
        let source: Value = serde_json::from_slice(
            &fs::read(activity_destination.join("change-source.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(source["review"]["activity"][0]["sha256"], sha256(&activity));
        fs::remove_dir_all(root).unwrap();
    }
}
