//! Single-file framing is sealed to the compiled Jira attachment protocol and a verified snapshot.
use super::*;
use crate::artifacts::ArtifactInput;
use bytes::Bytes;
use http_body::{Body, Frame, SizeHint};
use serde_json::json;
use std::io::{self, Read};
use std::pin::Pin;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::task::{Context, Poll};

const COMMAND: &str = "jira.attachment.upload";
pub struct MultipartFile {
    pub input: ArtifactInput,
    pub filename: String,
    pub media_type: String,
}
pub(super) fn requires_multipart(operation: &PreparedOperation) -> bool {
    operation.request().command == COMMAND
}
pub(super) struct UploadState {
    complete: AtomicBool,
    error: Mutex<Option<craft_core::storage::StoreError>>,
}
impl UploadState {
    pub(super) fn failure(&self, require_complete: bool) -> Option<CraftError> {
        use craft_core::storage::StoreError;
        let error = self.error.lock().ok().and_then(|v| *v);
        if let Some(error) = error {
            return Some(CraftError::new(
                match error {
                    StoreError::Cancelled => ErrorCode::Cancelled,
                    StoreError::DeadlineExceeded => ErrorCode::DeadlineExceeded,
                    StoreError::Conflict | StoreError::Corrupt => ErrorCode::VersionConflict,
                    _ => ErrorCode::WriteOutcomeUnknown,
                },
                "Multipart snapshot could not complete exact verified framing; reconcile the original upload before retrying.",
            ));
        }
        if require_complete && !self.complete.load(Ordering::SeqCst) {
            return Some(CraftError::new(
                ErrorCode::WriteOutcomeUnknown,
                "Native success arrived without complete verified multipart source framing; retain its unqualified response and reconcile.",
            ));
        }
        None
    }
}
pub(super) struct Parts {
    pub(super) request: Request,
    pub(super) call: NativeCall,
    pub(super) maximum: usize,
    pub(super) state: Arc<UploadState>,
}
fn metadata(file: &MultipartFile) -> Result<(), CraftError> {
    let media: Vec<_> = file.media_type.split('/').collect();
    if file.filename.is_empty()
        || file.filename.len() > 1024
        || file
            .filename
            .chars()
            .any(|c| c.is_control() || matches!(c, '/' | '\\'))
        || file.media_type.len() > 256
        || media.len() != 2
        || media.iter().any(|part| {
            part.is_empty()
                || !part
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&c))
        })
    {
        return Err(CraftError::field(
            "/multipart",
            "expected_bounded_inert_filename_and_unparameterized_media_type",
        ));
    }
    Ok(())
}
pub(super) fn seal(
    context: &HttpContext,
    operation: &PreparedOperation,
    call: &NativeCall,
    file: MultipartFile,
) -> Result<Parts, CraftError> {
    use craft_core::execution::RouteSegment;
    let route = context.route(call)?;
    if !requires_multipart(operation) || operation.request().target.provider!="jira" || !matches!(operation.effect(),EffectClass::Write)
        || !matches!(route.method,HttpMethod::Post) || route.path.len()!=6 || call.body.is_some()
        || !call.query.is_empty() || !call.repeated_query.is_empty()
        || !route.query_parameters.is_empty() || !route.continuation_parameters.is_empty() || !route.repeated_query_parameters.is_empty()
        || ["rest","api","3","issue"].iter().enumerate().any(|(index,expected)|!matches!(&route.path[index],RouteSegment::Literal(v) if v==expected))
        || !matches!(&route.path[4],RouteSegment::Parameter(v) if v=="issueIdOrKey")
        || !matches!(&route.path[5],RouteSegment::Literal(v) if v=="attachments") {
        return Err(denied());
    }
    metadata(&file)?;
    if !file.input.is_fresh() {
        return Err(CraftError::field(
            "/multipart",
            "expected_unconsumed_verified_snapshot",
        ));
    }
    let receipt = file.input.receipt();
    let input = &operation.request().input;
    if input["artifact_name"].as_str() != Some(receipt.name.as_str())
        || input["digest"].as_str() != Some(receipt.digest.as_str())
        || input["size_bytes"].as_u64() != Some(receipt.bytes)
        || input["filename"].as_str() != Some(file.filename.as_str())
        || input["media_type"].as_str() != Some(file.media_type.as_str())
        || input["issue_id"].as_str().is_none()
        || input["issue_id"].as_str()
            != call.path_parameters.get("issueIdOrKey").map(String::as_str)
    {
        return Err(CraftError::new(
            ErrorCode::PolicyDenied,
            "Multipart snapshot or issue route differs from its prepared operation input.",
        ));
    }

    if receipt.bytes > 1_073_741_824 {
        return Err(CraftError::field(
            "/multipart",
            "snapshot_exceeds_one_gibibyte",
        ));
    }
    let mut manifest = json!({"encoding":"multipart/form-data","field":"file","filename":file.filename,"media_type":file.media_type,"artifact":{"name":receipt.name,"digest":receipt.digest,"bytes":receipt.bytes,"source":receipt.source},"required_headers":{"X-Atlassian-Token":"no-check"}});
    let boundary = format!(
        "craft_{}",
        &craft_core::input::canonical_digest(&manifest)?[7..]
    );
    let quoted = file.filename.replace('"', "\\\"");
    let prefix = Bytes::from(format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{quoted}\"\r\nContent-Type: {}\r\n\r\n",
        file.media_type
    ));
    let suffix = Bytes::from(format!("\r\n--{boundary}--\r\n"));
    let length = receipt
        .bytes
        .checked_add(prefix.len() as u64)
        .and_then(|n| n.checked_add(suffix.len() as u64))
        .ok_or_else(internal)?;
    manifest["boundary"] = json!(boundary);
    manifest["content_length"] = json!(length);
    let mut frozen = call.clone();
    frozen.body = Some(manifest);
    let (mut request, maximum) = context.request(&frozen)?;
    request.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_str(&format!("multipart/form-data; boundary={boundary}"))
            .map_err(|_| internal())?,
    );
    request.headers_mut().insert(
        reqwest::header::CONTENT_LENGTH,
        HeaderValue::from_str(&length.to_string()).map_err(|_| internal())?,
    );
    request
        .headers_mut()
        .insert("x-atlassian-token", HeaderValue::from_static("no-check"));
    let state = Arc::new(UploadState {
        complete: AtomicBool::new(false),
        error: Mutex::new(None),
    });
    *request.body_mut() = Some(reqwest::Body::wrap(MultipartBody {
        input: file.input,
        prefix: Some(prefix),
        suffix: Some(suffix),
        remaining: length,
        state: state.clone(),
        done: false,
    }));
    Ok(Parts {
        request,
        call: frozen,
        maximum,
        state,
    })
}
struct MultipartBody {
    input: ArtifactInput,
    prefix: Option<Bytes>,
    suffix: Option<Bytes>,
    remaining: u64,
    state: Arc<UploadState>,
    done: bool,
}
impl MultipartBody {
    fn frame(&mut self, bytes: Bytes) -> Poll<Option<Result<Frame<Bytes>, io::Error>>> {
        self.remaining = self.remaining.saturating_sub(bytes.len() as u64);
        Poll::Ready(Some(Ok(Frame::data(bytes))))
    }
}
impl Body for MultipartBody {
    type Data = Bytes;
    type Error = io::Error;
    fn poll_frame(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, io::Error>>> {
        if self.done {
            return Poll::Ready(None);
        }
        if let Some(prefix) = self.prefix.take() {
            return self.frame(prefix);
        }
        let mut buffer = vec![0; 65_536];
        match self.input.read(&mut buffer) {
            Ok(0) => {
                if let Some(suffix) = self.suffix.take() {
                    self.state.complete.store(true, Ordering::SeqCst);
                    self.done = true;
                    self.frame(suffix)
                } else {
                    self.done = true;
                    Poll::Ready(None)
                }
            }
            Ok(count) => {
                buffer.truncate(count);
                self.frame(Bytes::from(buffer))
            }
            Err(error) => {
                let reason = error
                    .get_ref()
                    .and_then(|v| v.downcast_ref::<craft_core::storage::StoreError>())
                    .copied()
                    .unwrap_or(craft_core::storage::StoreError::Unavailable);
                if let Ok(mut stored) = self.state.error.lock() {
                    *stored = Some(reason);
                }
                self.done = true;
                Poll::Ready(Some(Err(error)))
            }
        }
    }
    fn is_end_stream(&self) -> bool {
        self.done
    }
    fn size_hint(&self) -> SizeHint {
        let mut hint = SizeHint::new();
        hint.set_exact(self.remaining);
        hint
    }
}
