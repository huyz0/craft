use super::*;
use craft_core::contracts::RequestState;
use craft_core::storage::SubmissionState;

pub struct PageWriteObservation {
    pub state: SubmissionState,
    pub resources: Vec<ResourceRef>,
    pub version: Option<u32>,
    pub error: Option<CraftError>,
}
fn failure(code: ErrorCode, state: RequestState) -> CraftError {
    let mut error = CraftError::new(
        code,
        "Confluence did not return complete qualified page publication evidence; no retry was performed.",
    );
    error.details.write_state = state;
    error.next_action = "Retain the operation receipt and any known page ID. Reconcile the original operation before another write; matching content alone does not prove attribution.".into();
    error
}
pub(super) fn unknown() -> CraftError {
    failure(ErrorCode::WriteOutcomeUnknown, RequestState::SentUnknown)
}
impl PreparedPageWrite {
    /// Interprets one response to this exact compiled write. Existing page identity alone is
    /// never evidence of an update. Incomplete successes retain known identities without replay.
    pub fn interpret(&self, response: &NativeResponse) -> PageWriteObservation {
        let mut result = PageWriteObservation {
            state: SubmissionState::Unknown,
            resources: vec![],
            version: None,
            error: None,
        };
        use PageWriteKind as K;
        let deletion = matches!(
            self.kind,
            K::Trash | K::CommentDelete | K::AttachmentTrash | K::FolderTrash | K::WhiteboardTrash
        );
        let success = match self.kind {
            K::Trash
            | K::CommentDelete
            | K::AttachmentTrash
            | K::FolderTrash
            | K::WhiteboardTrash => 204,
            K::CommentCreate | K::SpaceCreate => 201,
            // The v2 reference documents 200 for these creates; 201 is accepted as the same effect.
            K::FolderCreate | K::WhiteboardCreate if response.status == 201 => 201,
            // A property create may report 201; the confirmed body decides either way.
            K::WidthSet if response.status == 201 && self.version == 0 => 201,
            _ => 200,
        };
        if deletion && response.status == 204 {
            // The documented deletion success carries no body; the compiled identity is the effect.
            if let Some(id) = &self.id {
                result.resources.push(ResourceRef {
                    resource_type: self.kind.resource_type().into(),
                    id: id.clone(),
                    ..self.site.clone()
                });
                result.state = SubmissionState::Confirmed;
            }
            return result;
        }
        if response.status != success || deletion {
            let refusal = match self.kind {
                K::Create | K::SpaceCreate | K::FolderCreate | K::WhiteboardCreate => {
                    matches!(response.status, 400 | 401 | 404 | 413)
                }
                K::Update | K::CommentUpdate => matches!(response.status, 400 | 401 | 404 | 409),
                K::Trash
                | K::CommentCreate
                | K::CommentDelete
                | K::AttachmentTrash
                | K::FolderTrash
                | K::WhiteboardTrash => {
                    matches!(response.status, 400 | 401 | 404)
                }
                K::ContentStateSet | K::ContentStateClear => {
                    matches!(response.status, 400 | 401 | 403 | 404)
                }
                K::WidthSet => matches!(response.status, 400 | 401 | 403 | 404 | 409),
            };
            result.state = if refusal {
                SubmissionState::Failed
            } else {
                SubmissionState::Unknown
            };
            let code = match response.status {
                400 | 413 if refusal => ErrorCode::InputInvalid,
                401 if refusal => ErrorCode::AuthRequired,
                403 | 404 if refusal => ErrorCode::PermissionDenied,
                409 if refusal => ErrorCode::VersionConflict,
                429 => ErrorCode::RateLimited,
                _ => ErrorCode::WriteOutcomeUnknown,
            };
            result.error = Some(failure(
                code,
                if refusal {
                    RequestState::Failed
                } else {
                    RequestState::SentUnknown
                },
            ));
            return result;
        }
        let raw = &response.body;
        if matches!(self.kind, K::CommentCreate | K::CommentUpdate) {
            return self.interpret_comment(raw, result);
        }
        if self.kind == K::SpaceCreate {
            return self.interpret_space(raw, result);
        }
        if matches!(self.kind, K::FolderCreate | K::WhiteboardCreate) {
            return self.interpret_content(raw, result);
        }
        if self.kind == K::WidthSet {
            // The page is the compiled target; it is reported only once the property is confirmed.
            if self.width_confirmed(raw)
                && let Some(id) = &self.id
            {
                result.resources.push(ResourceRef {
                    resource_type: "page".into(),
                    id: id.clone(),
                    ..self.site.clone()
                });
                result.state = SubmissionState::Confirmed;
            } else {
                result.error = Some(failure(
                    ErrorCode::WriteOutcomeUnknown,
                    RequestState::SentUnknown,
                ));
            }
            return result;
        }
        if matches!(self.kind, K::ContentStateSet | K::ContentStateClear) {
            // The page identity is the compiled target; only the badge outcome needs evidence.
            if let Some(id) = &self.id {
                result.resources.push(ResourceRef {
                    resource_type: "page".into(),
                    id: id.clone(),
                    ..self.site.clone()
                });
            }
            if self.content_state_confirmed(raw) {
                result.state = SubmissionState::Confirmed;
            } else {
                result.error = Some(failure(
                    ErrorCode::WriteOutcomeUnknown,
                    RequestState::SentUnknown,
                ));
            }
            return result;
        }
        let id = raw
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| numeric(id));
        if let Some(id) = id
            && self.id.as_ref().is_none_or(|expected| expected == id)
            && raw.get("spaceId").and_then(Value::as_str) == Some(&self.space_id)
        {
            result.resources.push(ResourceRef {
                resource_type: "page".into(),
                id: id.into(),
                ..self.site.clone()
            });
        }
        let version = raw.pointer("/version/number").and_then(Value::as_u64);
        if !result.resources.is_empty()
            && raw.get("status").and_then(Value::as_str) == Some("current")
            && raw.get("title").and_then(Value::as_str) == Some(&self.title)
            && version == Some(u64::from(self.version))
            && raw.get("subtype").is_none_or(Value::is_null)
        {
            result.state = SubmissionState::Confirmed;
            result.version = Some(self.version);
        } else {
            result.error = Some(failure(
                ErrorCode::WriteOutcomeUnknown,
                RequestState::SentUnknown,
            ));
        }
        result
    }
    /// A comment write is confirmed only by a current comment with the compiled identity, page or
    /// parent association and next version. A known new identity survives incomplete evidence.
    fn interpret_comment(
        &self,
        raw: &Value,
        mut result: PageWriteObservation,
    ) -> PageWriteObservation {
        let id = raw
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| numeric(id));
        let page = raw.get("pageId").and_then(Value::as_str);
        let parent = raw.get("parentCommentId").and_then(Value::as_str);
        let associated = page == self.page_id.as_deref()
            || (self.parent_id.is_some() && parent == self.parent_id.as_deref());
        if let Some(id) = id
            && self.id.as_ref().is_none_or(|expected| expected == id)
            && associated
        {
            result.resources.push(ResourceRef {
                resource_type: "comment".into(),
                id: id.into(),
                ..self.site.clone()
            });
        }
        let version = raw.pointer("/version/number").and_then(Value::as_u64);
        let parent_ok = self.parent_id.is_none() || parent == self.parent_id.as_deref();
        if !result.resources.is_empty()
            && raw.get("status").and_then(Value::as_str) == Some("current")
            && version == Some(u64::from(self.version))
            && parent_ok
        {
            result.state = SubmissionState::Confirmed;
            result.version = Some(self.version);
        } else {
            result.error = Some(failure(
                ErrorCode::WriteOutcomeUnknown,
                RequestState::SentUnknown,
            ));
        }
        result
    }
}
impl PreparedPageWrite {
    /// A created space is confirmed by a numeric ID with the requested key and name.
    fn interpret_space(
        &self,
        raw: &Value,
        mut result: PageWriteObservation,
    ) -> PageWriteObservation {
        let id = raw
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| numeric(id));
        let key_matches = raw.get("key").and_then(Value::as_str) == self.parent_id.as_deref();
        if let Some(id) = id
            && key_matches
        {
            result.resources.push(ResourceRef {
                resource_type: "space".into(),
                id: id.into(),
                ..self.site.clone()
            });
        }
        if !result.resources.is_empty()
            && raw.get("name").and_then(Value::as_str) == Some(&self.title)
        {
            result.state = SubmissionState::Confirmed;
        } else {
            result.error = Some(failure(
                ErrorCode::WriteOutcomeUnknown,
                RequestState::SentUnknown,
            ));
        }
        result
    }
}
