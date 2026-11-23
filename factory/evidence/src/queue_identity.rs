use clap::ValueEnum;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Deserialize, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Operator,
    Runner,
    Verifier,
    Reviewer,
    Publisher,
    Intake,
    Diagnostician,
    ReleaseObserver,
    ProfileScheduler,
    MetricsReader,
    ContextReader,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IdentityConfig {
    pub id: String,
    pub role: Role,
    pub token_sha256: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "read_scope"
    )]
    pub verifier_repositories: Option<Vec<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "read_scope"
    )]
    pub context_repositories: Option<Vec<String>>,
}

impl IdentityConfig {
    pub fn repository_scope(&self) -> Result<Option<&[String]>, &'static str> {
        let verifier = self.verifier_repositories.as_deref();
        if verifier.is_some_and(|scope| self.role != Role::Verifier || !valid_scope(scope)) {
            return Err("a verifier scope needs 1 to 32 distinct repository keys");
        }
        let context = self.context_repositories.as_deref();
        match (self.role, context) {
            (Role::ContextReader, Some(scope)) if valid_scope(scope) => Ok(Some(scope)),
            (Role::ContextReader, _) => {
                Err("a context reader needs 1 to 32 distinct repository keys")
            }
            (_, Some(_)) => Err("a context scope is only allowed for a context reader"),
            (_, None) => Ok(verifier),
        }
    }
}

fn read_scope<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Vec<String>>, D::Error> {
    Vec::deserialize(deserializer).map(Some)
}

fn valid_scope(repositories: &[String]) -> bool {
    (1..=32).contains(&repositories.len())
        && repositories.iter().all(|repo| repository_key(repo))
        && repositories
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len()
            == repositories.len()
}

pub fn identifier(value: &str) -> bool {
    (1..=100).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

pub fn repository_key(value: &str) -> bool {
    value.split_once('/').is_some_and(|(owner, repo)| {
        [owner, repo].into_iter().all(|part| {
            (1..=100).contains(&part.len())
                && part
                    .as_bytes()
                    .first()
                    .is_some_and(u8::is_ascii_alphanumeric)
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        })
    })
}
