//! Complete native comment observation; a collection result is not human approval.
use super::{ConfluenceReadPolicy, PolicyContext, PreparedConfluenceRead};
use craft_core::contracts::{
    AuthMode, CanonicalRequest, CredentialCacheUpdate, GrantRef, RequestState, ResourceRef,
    Selection, validate_identity,
};
use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_core::input::canonical_digest;
use craft_core::pagination::{NativePage, NativePosition};
use craft_core::presence::InputField;
use craft_core::profiles::validate_profile_name;
use craft_core::storage::Clock;
use craft_provider_confluence::comments::CommentSummary;
use craft_provider_confluence::comments::paging::{COMMENT_LIST, COMMENT_REPLIES};
use craft_runtime::auth::{ServiceOAuthClient, VerifiedServiceAccess};
use craft_runtime::confluence_read_policy::{
    ConfluenceReadOperation as Operation, ConfluenceReadPolicyRecord,
};
use craft_runtime::profiles::ProfileStore;
use craft_runtime::secrets::LockedCredentials;
use craft_runtime::transport::TlsPolicy;
use craft_workflows::review::{CommentBody, CommentInventory, CommentObservation, Coverage};
use serde_json::{Value, json};
use std::cell::Cell;
use std::collections::{BTreeSet, VecDeque};
use std::sync::Arc;

const MAX_COMMENTS: usize = 1024;
const MAX_BODY_BYTES: usize = 65_536;
const MAX_TOTAL_BYTES: usize = 8_388_608;
const MAX_PAGES: usize = 2048;
const MAX_AGE_MS: u64 = 30_000;

fn refused(message: &str) -> CraftError {
    CraftError::new(ErrorCode::PolicyDenied, message)
}
fn malformed() -> CraftError {
    CraftError::new(
        ErrorCode::RemoteUnavailable,
        "Comment traversal did not establish complete current page-bound coverage.",
    )
}
fn numeric(value: &str) -> bool {
    value
        .parse::<i64>()
        .is_ok_and(|n| n > 0 && n.to_string() == value)
}

/// Prepared from native profile/policy state, never reconstructed from evidence JSON.
pub struct PreparedInventory {
    page: ResourceRef,
    policy: ConfluenceReadPolicyRecord,
    root: PreparedConfluenceRead,
    reply_guard: PreparedConfluenceRead,
    request_id: String,
}
/// Cache effects survive failed reads. Only Ok carries a complete inventory; neither grants approval.
pub struct InventoryResult {
    pub inventory: Result<CommentInventory, CraftError>,
    pub credential_cache_update: Option<CredentialCacheUpdate>,
    pub execution_actor: Option<String>,
}
impl PreparedInventory {
    pub fn prepare(
        page: ResourceRef,
        profile: &str,
        request_id: &str,
        store: &ProfileStore,
        control: &InvocationControl,
    ) -> Result<Self, CraftError> {
        validate_profile_name(profile)?;
        page.validate()?;
        validate_identity("/request_id", request_id)?;
        control.check()?;
        if page.provider != "confluence"
            || page.deployment != "cloud"
            || page.resource_type != "page"
            || !numeric(&page.id)
        {
            return Err(CraftError::field(
                "/page",
                "expected_exact_confluence_cloud_page",
            ));
        }
        let selected = store.get(profile, control)?;
        if selected.provider != "confluence"
            || selected.deployment != "cloud"
            || selected.auth_mode != AuthMode::Service
        {
            return Err(refused(
                "Complete comment observation requires a named Confluence Cloud service profile.",
            ));
        }
        let policy=store.confluence_read_policy(profile,control)?.ok_or_else(||refused("Complete comment observation requires explicit root and reply read allowances."))?;
        if page.tenant_ref() != policy.target.tenant_ref() {
            return Err(refused(
                "Review page differs from the configured read site.",
            ));
        }
        for op in [Operation::CommentList, Operation::CommentReplies] {
            store.verify_confluence_read_policy(&policy, op, control)?;
        }
        let root = prepare_read(&page, &policy, request_id, None)?;
        let reply_guard = prepare_read(&page, &policy, request_id, Some("1"))?;
        Ok(Self {
            page,
            policy,
            root,
            reply_guard,
            request_id: request_id.into(),
        })
    }
    pub fn grant(&self) -> &GrantRef {
        &self.policy.grant
    }
    fn check_policy(
        &self,
        store: &ProfileStore,
        control: &InvocationControl,
    ) -> Result<(), CraftError> {
        for op in [Operation::CommentList, Operation::CommentReplies] {
            store.verify_confluence_read_policy(&self.policy, op, control)?;
        }
        Ok(())
    }
    pub fn execute_cached(
        &self,
        store: &ProfileStore,
        client: &ServiceOAuthClient,
        locked: &LockedCredentials<'_>,
        tls: TlsPolicy,
        control: InvocationControl,
    ) -> InventoryResult {
        self.execute_cached_with_check(store, client, locked, tls, control, || Ok(()))
    }
    pub(crate) fn check_access(
        &self,
        access: &VerifiedServiceAccess,
        control: &InvocationControl,
    ) -> Result<(), CraftError> {
        self.root.check_access(access, control, &|| Ok(()))?;
        self.reply_guard.check_access(access, control, &|| Ok(()))
    }
    /// Adds workflow checks without replacing the captured read policy or native access checks.
    pub fn execute_cached_with_check(
        &self,
        store: &ProfileStore,
        client: &ServiceOAuthClient,
        locked: &LockedCredentials<'_>,
        tls: TlsPolicy,
        control: InvocationControl,
        check_workflow: impl Fn() -> Result<(), CraftError>,
    ) -> InventoryResult {
        let budget = Budget::new(client.clock().clone());
        let before = (|| {
            budget.check(&control)?;
            check_workflow()?;
            self.check_policy(store, &control)?;
            if locked.grant() != self.policy.grant {
                return Err(refused(
                    "Locked credentials differ from the captured inventory grant.",
                ));
            }
            client.for_invocation(control.clone())
        })();
        let client = match before {
            Ok(client) => client,
            Err(error) => {
                return InventoryResult {
                    inventory: Err(error),
                    credential_cache_update: None,
                    execution_actor: None,
                };
            }
        };
        let access = match client.open_service_access(locked, &self.policy.target.tenant_ref()) {
            Ok(access) => access,
            Err(failure) => {
                let cache = failure
                    .credential_update
                    .as_ref()
                    .map(|r| CredentialCacheUpdate {
                        grant: locked.grant(),
                        write_state: RequestState::Confirmed,
                        cleanup_pending: Some(r.cleanup_pending),
                    })
                    .or_else(|| {
                        failure
                            .credential_write_state
                            .map(|state| CredentialCacheUpdate {
                                grant: locked.grant(),
                                write_state: state,
                                cleanup_pending: None,
                            })
                    });
                let mut error = failure.error;
                error.details.write_state = RequestState::NotSent;
                return InventoryResult {
                    inventory: Err(error),
                    credential_cache_update: cache,
                    execution_actor: None,
                };
            }
        };
        let cache = access.cache_update().map(|r| CredentialCacheUpdate {
            grant: access.grant().clone(),
            write_state: RequestState::Confirmed,
            cleanup_pending: Some(r.cleanup_pending),
        });
        let actor = Some(access.identity().actor().to_owned());
        let mut source = NativeSource {
            owner: self,
            store,
            access: &access,
            tls,
            control: &control,
            budget: &budget,
            reply: None,
            check_workflow: &check_workflow,
        };
        let inventory = collect(&mut source, &self.page);
        InventoryResult {
            inventory,
            credential_cache_update: cache,
            execution_actor: actor,
        }
    }
}
fn prepare_read(
    page: &ResourceRef,
    policy: &ConfluenceReadPolicyRecord,
    request_id: &str,
    parent: Option<&str>,
) -> Result<PreparedConfluenceRead, CraftError> {
    PreparedConfluenceRead::prepare(
        CanonicalRequest {
            command: if parent.is_some() {
                COMMENT_REPLIES
            } else {
                COMMENT_LIST
            }
            .into(),
            target: policy.target.clone(),
            input: match parent {
                Some(id) => json!({"page_id":page.id,"parent_comment_id":id,"page_size":25}),
                None => json!({"page_id":page.id,"page_size":25}),
            },
            selection: Selection {
                fields: vec![
                    "body_storage".into(),
                    "revision".into(),
                    "parent_comment_id".into(),
                ],
                max_bytes: 1_048_576,
                ..Default::default()
            },
            preconditions: None,
        },
        request_id,
        ConfluenceReadPolicy {
            target: policy.target.clone(),
            grant: policy.grant.clone(),
            expected_actor: policy.expected_actor.clone(),
            enabled: policy.enabled,
            allowed_commands: policy
                .operations
                .iter()
                .map(|op| op.command().into())
                .collect(),
        },
    )
}
struct Budget {
    clock: Arc<dyn Clock>,
    wall: u64,
    mono: u64,
    last_wall: Cell<u64>,
    last_mono: Cell<u64>,
}
impl Budget {
    fn new(clock: Arc<dyn Clock>) -> Self {
        let wall = clock.unix_millis();
        let mono = clock.monotonic_millis();
        Self {
            clock,
            wall,
            mono,
            last_wall: Cell::new(wall),
            last_mono: Cell::new(mono),
        }
    }
    fn check(&self, control: &InvocationControl) -> Result<(), CraftError> {
        control.check()?;
        let wall = self.clock.unix_millis();
        let mono = self.clock.monotonic_millis();
        if wall < self.last_wall.get()
            || mono < self.last_mono.get()
            || wall.saturating_sub(self.wall) >= MAX_AGE_MS
            || mono.saturating_sub(self.mono) >= MAX_AGE_MS
        {
            return Err(refused(
                "Comment inventory freshness expired or a clock moved backward.",
            ));
        }
        self.last_wall.set(wall);
        self.last_mono.set(mono);
        Ok(())
    }
}
trait PageReader {
    fn check(&self) -> Result<(), CraftError>;
    fn read(
        &mut self,
        parent: Option<&str>,
        position: Option<&NativePosition>,
    ) -> Result<NativePage, CraftError>;
}
struct NativeSource<'a> {
    owner: &'a PreparedInventory,
    store: &'a ProfileStore,
    access: &'a VerifiedServiceAccess,
    tls: TlsPolicy,
    control: &'a InvocationControl,
    budget: &'a Budget,
    reply: Option<(String, PreparedConfluenceRead)>,
    check_workflow: &'a dyn Fn() -> Result<(), CraftError>,
}
impl PageReader for NativeSource<'_> {
    fn check(&self) -> Result<(), CraftError> {
        self.budget.check(self.control)?;
        (self.check_workflow)()?;
        self.owner.check_policy(self.store, self.control)?;
        self.owner
            .root
            .check_access(self.access, self.control, &|| Ok(()))?;
        self.owner
            .reply_guard
            .check_access(self.access, self.control, &|| Ok(()))?;
        self.budget.check(self.control)?;
        self.owner.check_policy(self.store, self.control)?;
        (self.check_workflow)()?;
        self.control.check()
    }
    fn read(
        &mut self,
        parent: Option<&str>,
        position: Option<&NativePosition>,
    ) -> Result<NativePage, CraftError> {
        self.check()?;
        if let Some(id) = parent
            && self.reply.as_ref().is_none_or(|(current, _)| current != id)
        {
            self.reply = Some((
                id.into(),
                prepare_read(
                    &self.owner.page,
                    &self.owner.policy,
                    &self.owner.request_id,
                    Some(id),
                )?,
            ));
        }
        let selected = if parent.is_some() {
            &self.reply.as_ref().ok_or_else(malformed)?.1
        } else {
            &self.owner.root
        };
        let mut native = self.access.read_context(
            &selected.registry,
            &selected.operation,
            self.tls.clone(),
            self.control.clone(),
        )?;
        let check = || self.check();
        let mut context = PolicyContext {
            native: &mut native,
            check: &check,
        };
        let page =
            craft_provider_confluence::read_page(&selected.operation, &mut context, position)?;
        self.check()?;
        Ok(page)
    }
}
fn collect(
    source: &mut dyn PageReader,
    page: &ResourceRef,
) -> Result<CommentInventory, CraftError> {
    let mut queue = VecDeque::from([None]);
    let mut ids = BTreeSet::new();
    let mut comments: Vec<CommentObservation> = Vec::new();
    let mut total_bytes = 0usize;
    let mut pages = 0usize;
    while let Some(parent) = queue.pop_front() {
        let mut position = None;
        let mut cursors = BTreeSet::new();
        loop {
            source.check()?;
            pages += 1;
            if pages > MAX_PAGES {
                return Err(refused("Comment inventory native-page limit exceeded."));
            }
            let native = source.read(parent.as_deref(), position.as_ref())?;
            source.check()?;
            if let Some(parent) = parent.as_deref() {
                let observed = comments
                    .iter()
                    .find(|c| c.id == parent)
                    .ok_or_else(malformed)?;
                let proof = native.context.as_ref().ok_or_else(malformed)?;
                let current = &proof["parent"];
                if proof["page_id"] != page.id
                    || proof["parent_comment_id"] != parent
                    || proof["coverage"] != "direct_replies"
                    || current["id"] != observed.id
                    || current["page_id"] != page.id
                    || current["status"] != observed.status
                    || current["version"] != observed.version
                    || current["version_author_id"] != observed.version_author_id
                    || current["version_created_at"] != observed.version_created_at
                {
                    return Err(CraftError::new(
                        ErrorCode::VersionConflict,
                        "A direct current parent revision contradicts an inventoried comment.",
                    ));
                }
            }
            if native.items.len() > 25 {
                return Err(malformed());
            }
            for item in native.items {
                source.check()?;
                let comment = observation(item, page, parent.as_deref())?;
                if !ids.insert(comment.id.clone()) || comments.len() >= MAX_COMMENTS {
                    return Err(refused(
                        "Comment inventory contains duplicate identities or exceeds its comment limit.",
                    ));
                }
                let bytes = serde_json::to_vec(&comment.body)
                    .map_err(|_| malformed())?
                    .len();
                total_bytes = total_bytes.checked_add(bytes).ok_or_else(malformed)?;
                if bytes > MAX_BODY_BYTES || total_bytes > MAX_TOTAL_BYTES {
                    return Err(refused("Comment inventory body limits exceeded."));
                }
                queue.push_back(Some(comment.id.clone()));
                comments.push(comment);
            }
            match native.next {
                None => break,
                Some(next) => {
                    next.validate()?;
                    if next.0.len() != 1
                        || next.0.get("cursor").is_none_or(|s| s.is_empty())
                        || !cursors.insert(canonical_digest(&json!(next.0))?)
                    {
                        return Err(CraftError::new(
                            ErrorCode::PaginationStalled,
                            "Comment inventory continuation repeated or is invalid.",
                        ));
                    }
                    position = Some(next);
                }
            }
        }
    }
    source.check()?;
    Ok(CommentInventory {
        coverage: Coverage::Complete,
        comments,
    })
}
fn observation(
    item: Value,
    page: &ResourceRef,
    parent: Option<&str>,
) -> Result<CommentObservation, CraftError> {
    let value: CommentSummary = serde_json::from_value(item).map_err(|_| malformed())?;
    let parent_matches = match (&value.parent_comment_id, parent) {
        (InputField::Null, None) => true,
        (InputField::Value(id), Some(expected)) => id == expected,
        _ => false,
    };
    if !numeric(&value.id)
        || value.page_id != page.id
        || value.status != "current"
        || value.version == 0
        || value.version > i32::MAX as u32
        || !parent_matches
        || value.body_state != "returned"
        || !value.fields_not_returned.is_empty()
        || !value.body_atlas_doc_format.is_omitted()
    {
        return Err(malformed());
    }
    validate_identity("/comment/version_author_id", &value.version_author_id)
        .map_err(|_| malformed())?;
    if value.version_created_at.is_empty()
        || value.version_created_at.len() > 4096
        || value.version_created_at.chars().any(char::is_control)
    {
        return Err(malformed());
    }
    let InputField::Value(body) = value.body_storage else {
        return Err(malformed());
    };
    let InputField::Value(revision) = value.revision else {
        return Err(malformed());
    };
    if body.contains('\0')
        || revision.version != value.version
        || revision.representation != "storage"
        || revision.digest
            != canonical_digest(
                &json!({"id":value.id,"page_id":value.page_id,"parent_comment_id":parent,"status":"current","version":value.version,"version_author_id":value.version_author_id,"version_created_at":value.version_created_at,"representation":"storage","body":body}),
            )?
    {
        return Err(malformed());
    }
    Ok(CommentObservation {
        id: value.id,
        page: page.clone(),
        status: value.status,
        version: value.version,
        version_author_id: value.version_author_id,
        version_created_at: value.version_created_at,
        body: CommentBody::Storage(body),
    })
}

#[cfg(test)]
mod tests;
