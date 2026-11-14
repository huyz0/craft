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
        if self.kind == PageWriteKind::Trash && response.status == 204 {
            // The documented trash success carries no body; the compiled identity is the effect.
            if let Some(id) = &self.id {
                result.resources.push(ResourceRef {
                    resource_type: "page".into(),
                    id: id.clone(),
                    ..self.site.clone()
                });
                result.state = SubmissionState::Confirmed;
            }
            return result;
        }
        if response.status != 200 || self.kind == PageWriteKind::Trash {
            let refusal = match self.kind {
                PageWriteKind::Create => matches!(response.status, 400 | 401 | 404 | 413),
                PageWriteKind::Update => matches!(response.status, 400 | 401 | 404 | 409),
                PageWriteKind::Trash => matches!(response.status, 400 | 401 | 404),
            };
            result.state = if refusal {
                SubmissionState::Failed
            } else {
                SubmissionState::Unknown
            };
            let code = match response.status {
                400 | 413 if refusal => ErrorCode::InputInvalid,
                401 if refusal => ErrorCode::AuthRequired,
                404 if refusal => ErrorCode::PermissionDenied,
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
}
