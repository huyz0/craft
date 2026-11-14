use super::*;
use crate::comments::adf::Document;
use craft_core::presence::InputField;

#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Ownership {
    #[default]
    Own,
    Any,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "mode", rename_all = "lowercase", deny_unknown_fields)]
pub enum VisibilityChange {
    Project {},
    Group { identifier: String },
    Role { identifier: String },
}
impl VisibilityChange {
    fn validate(&self) -> Result<(), CraftError> {
        let id = match self {
            Self::Project {} => return Ok(()),
            Self::Group { identifier } | Self::Role { identifier } => identifier,
        };
        if id.is_empty() || id.len() > 512 || id.trim() != id || id.chars().any(char::is_control) {
            return Err(CraftError::field(
                "/visibility",
                "expected_group_id_or_role_name",
            ));
        }
        Ok(())
    }
    pub(super) fn native(&self) -> Value {
        match self {
            Self::Project {} => Value::Null,
            Self::Group { identifier } => json!({"type":"group","identifier":identifier}),
            Self::Role { identifier } => json!({"type":"role","identifier":identifier}),
        }
    }
    pub(super) fn matches(&self, visibility: Option<&super::super::Visibility>) -> bool {
        match (self, visibility) {
            (Self::Project {}, None) => true,
            (Self::Group { identifier }, Some(v)) => {
                v.kind == super::super::VisibilityType::Group
                    && v.identifier.as_ref() == Some(identifier)
            }
            (Self::Role { identifier }, Some(v)) => {
                v.kind == super::super::VisibilityType::Role
                    && (v.identifier.as_ref() == Some(identifier)
                        || v.identifier.is_none() && v.value.as_ref() == Some(identifier))
            }
            _ => false,
        }
    }
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateInput {
    #[schemars(regex(pattern = "^[1-9][0-9]{0,18}$"))]
    pub issue_id: String,
    pub body: Document,
    pub visibility: VisibilityChange,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateInput {
    #[schemars(regex(pattern = "^[1-9][0-9]{0,18}$"))]
    pub issue_id: String,
    #[schemars(regex(pattern = "^[1-9][0-9]{0,18}$"))]
    pub comment_id: String,
    #[schemars(length(min = 71, max = 71), regex(pattern = "^sha256:[0-9a-f]{64}$"))]
    pub expected_revision: String,
    #[serde(default)]
    pub ownership: Ownership,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    #[schemars(with = "Document")]
    pub body: InputField<Document>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    #[schemars(with = "VisibilityChange")]
    pub visibility: InputField<VisibilityChange>,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeleteInput {
    #[schemars(regex(pattern = "^[1-9][0-9]{0,18}$"))]
    pub issue_id: String,
    #[schemars(regex(pattern = "^[1-9][0-9]{0,18}$"))]
    pub comment_id: String,
    #[schemars(length(min = 71, max = 71), regex(pattern = "^sha256:[0-9a-f]{64}$"))]
    pub expected_revision: String,
    #[serde(default)]
    pub ownership: Ownership,
}
pub(super) enum Intent {
    Create(CreateInput),
    Update(UpdateInput),
    Delete(DeleteInput),
}
impl Intent {
    pub(super) fn issue(&self) -> &str {
        match self {
            Self::Create(v) => &v.issue_id,
            Self::Update(v) => &v.issue_id,
            Self::Delete(v) => &v.issue_id,
        }
    }
    pub(super) fn comment(&self) -> Option<(&str, &str, Ownership)> {
        match self {
            Self::Create(_) => None,
            Self::Update(v) => Some((&v.comment_id, &v.expected_revision, v.ownership)),
            Self::Delete(v) => Some((&v.comment_id, &v.expected_revision, v.ownership)),
        }
    }
    pub(super) fn payload(&self) -> Result<Option<Value>, CraftError> {
        Ok(match self {
            Self::Create(v) => Some(json!({"body":v.body,"visibility":v.visibility.native()})),
            Self::Delete(_) => None,
            Self::Update(v) => {
                let mut body = json!({});
                if let InputField::Value(value) = &v.body {
                    body["body"] = serde_json::to_value(value).map_err(|_| invalid())?;
                }
                if let InputField::Value(value) = &v.visibility {
                    body["visibility"] = value.native();
                }
                Some(body)
            }
        })
    }
    pub(super) fn desired(&self, record: &super::super::CommentRecord) -> Result<bool, CraftError> {
        let body_matches = |doc: &Document| -> Result<bool, CraftError> {
            Ok(record.body.as_ref() == Some(&serde_json::to_value(doc).map_err(|_| invalid())?))
        };
        Ok(match self {
            Self::Create(v) => {
                body_matches(&v.body)? && v.visibility.matches(record.visibility.as_ref())
            }
            Self::Update(v) => {
                (!matches!(&v.body, InputField::Value(body) if !body_matches(body)?))
                    && !matches!(&v.visibility, InputField::Value(vis) if !vis.matches(record.visibility.as_ref()))
            }
            Self::Delete(_) => true,
        })
    }
}
fn invalid() -> CraftError {
    CraftError::field("/input", "invalid_comment_mutation")
}
pub(super) fn intent(request: &CanonicalRequest) -> Result<Intent, CraftError> {
    crate::created_issue::validate_target(&request.target)?;
    if !super::super::numeric(&request.target.id)
        || request.preconditions.is_some()
        || request.selection.cursor.is_some()
        || !request.selection.fields.is_empty()
    {
        return Err(CraftError::field(
            "/request",
            "exact_complete_comment_mutation_required",
        ));
    }
    let command = write_command(&request.command).ok_or_else(invalid)?;
    let parsed = match command {
        CREATE => {
            Intent::Create(serde_json::from_value(request.input.clone()).map_err(|_| invalid())?)
        }
        UPDATE => {
            Intent::Update(serde_json::from_value(request.input.clone()).map_err(|_| invalid())?)
        }
        _ => Intent::Delete(serde_json::from_value(request.input.clone()).map_err(|_| invalid())?),
    };
    if !super::super::numeric(parsed.issue()) {
        return Err(invalid());
    }
    if let Some((comment, revision, _)) = parsed.comment()
        && (!super::super::numeric(comment)
            || !revision.strip_prefix("sha256:").is_some_and(|v| {
                v.len() == 64
                    && v.bytes()
                        .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
            }))
    {
        return Err(invalid());
    }
    match &parsed {
        Intent::Create(v) => {
            v.body.validate()?;
            v.visibility.validate()?;
        }
        Intent::Update(v) => {
            if matches!(v.body, InputField::Null)
                || matches!(v.visibility, InputField::Null)
                || v.body.is_omitted() && v.visibility.is_omitted()
            {
                return Err(invalid());
            }
            if let InputField::Value(body) = &v.body {
                body.validate()?;
            }
            if let InputField::Value(visibility) = &v.visibility {
                visibility.validate()?;
            }
        }
        Intent::Delete(_) => {}
    }
    Ok(parsed)
}
