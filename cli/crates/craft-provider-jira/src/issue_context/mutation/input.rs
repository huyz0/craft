use super::*;
use craft_core::presence::InputField;

#[derive(Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LabelDelta {
    #[serde(default)]
    pub add: Vec<String>,
    #[serde(default)]
    pub remove: Vec<String>,
}

#[derive(Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueChanges {
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub summary: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub description: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub environment: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub labels: InputField<Vec<String>>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub label_delta: InputField<LabelDelta>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub priority_id: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub resolution_id: InputField<String>,
    #[serde(default)]
    pub custom_fields: BTreeMap<String, Value>,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueEditInput {
    pub issue: String,
    pub expected: IssueState,
    pub changes: IssueChanges,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueTransitionInput {
    pub issue: String,
    pub expected: IssueState,
    pub transition_id: String,
    pub expected_status_id: String,
    pub changes: IssueChanges,
}

pub(super) struct Intent {
    pub expected: IssueState,
    pub changes: IssueChanges,
    pub transition: Option<(String, String)>,
}

fn labels(values: &[String]) -> bool {
    values.len() <= 100
        && values.iter().all(|value| {
            !value.is_empty()
                && value.len() <= 255
                && !value.chars().any(|c| c.is_whitespace() || c.is_control())
        })
        && values.iter().collect::<BTreeSet<_>>().len() == values.len()
}

impl IssueChanges {
    pub(super) fn values(&self) -> Result<BTreeMap<String, Value>, CraftError> {
        let mut out = self.custom_fields.clone();
        if out.len() > 64
            || out
                .keys()
                .any(|id| !id.strip_prefix("customfield_").is_some_and(numeric_id))
        {
            return Err(invalid(
                "/changes/custom_fields",
                "expected_bounded_custom_field_ids",
            ));
        }
        for (id, field, max) in [
            ("summary", &self.summary, 255),
            ("description", &self.description, 32768),
            ("environment", &self.environment, 32768),
        ] {
            match field {
                InputField::Omitted => {}
                InputField::Null if id != "summary" => {
                    out.insert(id.into(), Value::Null);
                }
                InputField::Value(value)
                    if value.len() <= max
                        && !value.contains('\0')
                        && (id != "summary"
                            || (!value.trim().is_empty()
                                && !value.chars().any(char::is_control))) =>
                {
                    out.insert(id.into(), json!(value));
                }
                _ => return Err(invalid(&format!("/changes/{id}"), "invalid_text_or_clear")),
            }
        }
        for (id, field) in [
            ("priority", &self.priority_id),
            ("resolution", &self.resolution_id),
        ] {
            match field {
                InputField::Omitted => {}
                InputField::Null => {
                    out.insert(id.into(), Value::Null);
                }
                InputField::Value(value) if numeric_id(value) => {
                    out.insert(id.into(), json!({"id":value}));
                }
                _ => return Err(invalid("/changes", "expected_numeric_option_id")),
            }
        }
        match &self.labels {
            InputField::Omitted => {}
            InputField::Null => {
                out.insert("labels".into(), Value::Null);
            }
            InputField::Value(values) if labels(values) => {
                out.insert("labels".into(), json!(values));
            }
            _ => return Err(invalid("/changes/labels", "invalid_unique_labels")),
        }
        match &self.label_delta {
            InputField::Omitted => {}
            InputField::Value(delta)
                if self.labels.is_omitted()
                    && labels(&delta.add)
                    && labels(&delta.remove)
                    && !(delta.add.is_empty() && delta.remove.is_empty())
                    && delta.add.iter().all(|value| !delta.remove.contains(value)) => {}
            _ => {
                return Err(invalid(
                    "/changes/label_delta",
                    "expected_disjoint_nonempty_delta_without_replacement",
                ));
            }
        }
        if out.len() > 64 {
            return Err(invalid("/changes", "too_many_fields"));
        }
        Ok(out)
    }
}

pub(super) fn intent(request: &CanonicalRequest) -> Result<Intent, CraftError> {
    if request.preconditions.is_some() {
        return Err(invalid(
            "/preconditions",
            "use_explicit_expected_issue_state_not_server_cas",
        ));
    }
    let bytes =
        serde_json::to_vec(&request.input).map_err(|_| invalid("/input", "cannot_encode_input"))?;
    craft_core::input::parse_json(&bytes, 65_536)?;
    let (issue, expected, changes, transition) = match request.command.as_str() {
        EDIT_ISSUE => {
            let value: IssueEditInput = serde_json::from_value(request.input.clone())
                .map_err(|_| invalid("/input", "invalid_edit_input"))?;
            (value.issue, value.expected, value.changes, None)
        }
        TRANSITION_ISSUE => {
            let value: IssueTransitionInput = serde_json::from_value(request.input.clone())
                .map_err(|_| invalid("/input", "invalid_transition_input"))?;
            validate_identity("/transition_id", &value.transition_id)?;
            if !numeric_id(&value.expected_status_id) {
                return Err(invalid("/expected_status_id", "expected_numeric_status_id"));
            }
            (
                value.issue,
                value.expected,
                value.changes,
                Some((value.transition_id, value.expected_status_id)),
            )
        }
        _ => return Err(invalid("/command", "expected_issue_edit_or_transition")),
    };
    if !numeric_id(&issue) || expected.id != issue {
        return Err(invalid("/issue", "expected_resolved_numeric_issue_id"));
    }
    super::super::validate_target(&request.target, &issue)?;
    IssueObservation {
        requested_issue: issue,
        resolved_issue: request.target.clone(),
        resolution: IdentityResolution::ExactId,
        state: expected.clone(),
    }
    .validate_for_target(&request.target)
    .map_err(|_| invalid("/expected", "invalid_expected_issue_state"))?;
    let values = changes.values()?;
    if transition.is_none() && values.is_empty() && changes.label_delta.is_omitted() {
        return Err(invalid("/changes", "expected_nonempty_edit"));
    }
    Ok(Intent {
        expected,
        changes,
        transition,
    })
}
