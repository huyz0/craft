//! Encodes one result without discarding state-changing outcome evidence.

use craft_core::contracts::Envelope;
use craft_core::error::{CraftError, ErrorCode};

#[derive(Clone, Copy)]
pub enum OutputPolicy {
    Bounded,
    PreserveEvidence,
}

pub struct EncodedOutput {
    pub bytes: Vec<u8>,
    pub exit: u8,
}

/// State-changing handlers must preflight their complete result before mutation.
/// If that invariant fails, retain the known outcome rather than imply no effect.
pub fn encode(
    mut envelope: Envelope,
    maximum: usize,
    policy: OutputPolicy,
) -> Result<EncodedOutput, serde_json::Error> {
    let mut exit = if envelope.ok {
        0
    } else {
        envelope.error.as_ref().map_or_else(
            || ErrorCode::Internal.exit_code(),
            |error| error.code.exit_code(),
        )
    };
    let mut bytes = serde_json::to_vec(&envelope)?;
    if bytes.len().saturating_add(1) > maximum {
        match policy {
            OutputPolicy::Bounded => {
                let mut error =
                    CraftError::new(ErrorCode::OutputBudgetTooSmall, "Output budget too small.");
                error.next_action = "Increase --max-bytes.".into();
                envelope = Envelope::failure(&envelope.command, envelope.request_id, error);
                exit = ErrorCode::OutputBudgetTooSmall.exit_code();
            }
            OutputPolicy::PreserveEvidence => {
                envelope.warnings.push(
                    "Output exceeded --max-bytes; the recorded write outcome is preserved. Reconcile before repeating."
                        .into(),
                );
                if exit == 0 {
                    exit = ErrorCode::OutputBudgetTooSmall.exit_code();
                }
            }
        }
        bytes = serde_json::to_vec(&envelope)?;
    }
    bytes.push(b'\n');
    Ok(EncodedOutput { bytes, exit })
}

#[cfg(test)]
mod tests {
    use super::*;
    use craft_core::contracts::{Page, RequestState};
    use serde_json::json;

    #[test]
    fn complete_failure_retains_recovery_rate_warnings_page_and_exit()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut error = CraftError::new(ErrorCode::WriteOutcomeUnknown, "Outcome unknown.");
        error.details.write_state = RequestState::SentUnknown;
        let mut envelope = Envelope::failure_with_data(
            "auth.provision",
            "original-request".into(),
            error,
            json!({"credential_write_state":"confirmed","cleanup_pending":true,"retry_after":{"delay_seconds":7}}),
        );
        envelope.warnings.push("Retain this receipt.".into());
        envelope.page = Some(Page {
            returned: 0,
            has_more: true,
            next_cursor: Some("opaque".into()),
            truncated: false,
            stop_reason: "budget".into(),
            consistency: "live".into(),
        });
        let expected = serde_json::to_value(&envelope)?;
        let output = encode(envelope, 16_384, OutputPolicy::PreserveEvidence)?;
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&output.bytes)?,
            expected
        );
        assert_eq!(output.exit, ErrorCode::WriteOutcomeUnknown.exit_code());
        assert_eq!(output.bytes.last(), Some(&b'\n'));
        assert_eq!(
            output.bytes.iter().filter(|byte| **byte == b'\n').count(),
            1
        );
        Ok(())
    }

    #[test]
    fn oversized_write_evidence_survives_while_reads_return_a_budget_error()
    -> Result<(), Box<dyn std::error::Error>> {
        for failed in [false, true] {
            let data = json!({"credential_write_state":"confirmed","receipt":"x".repeat(1_000)});
            let envelope = if failed {
                Envelope::failure_with_data(
                    "auth.provision",
                    "request".into(),
                    CraftError::new(ErrorCode::WriteOutcomeUnknown, "Retain outcome."),
                    data.clone(),
                )
            } else {
                Envelope::success("auth.provision", "request".into(), data.clone())
            };
            let output = encode(envelope, 512, OutputPolicy::PreserveEvidence)?;
            let result: serde_json::Value = serde_json::from_slice(&output.bytes)?;
            assert_eq!(result["data"], data);
            assert_eq!(result["ok"], !failed);
            assert_eq!(result["request_id"], "request");
            assert_eq!(
                result["warnings"]
                    .as_array()
                    .ok_or("Missing warning")?
                    .len(),
                1
            );
            assert_eq!(
                output.exit,
                if failed {
                    ErrorCode::WriteOutcomeUnknown.exit_code()
                } else {
                    ErrorCode::OutputBudgetTooSmall.exit_code()
                }
            );
        }
        let output = encode(
            Envelope::success(
                "version",
                "request".into(),
                json!({"text":"x".repeat(1_000)}),
            ),
            512,
            OutputPolicy::Bounded,
        )?;
        let result: serde_json::Value = serde_json::from_slice(&output.bytes)?;
        assert_eq!(result["ok"], false);
        assert_eq!(result["error"]["code"], "OUTPUT_BUDGET_TOO_SMALL");
        assert!(output.bytes.len() <= 512);
        assert_eq!(output.exit, ErrorCode::OutputBudgetTooSmall.exit_code());
        Ok(())
    }
}
