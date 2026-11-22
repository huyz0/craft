use std::{path::Path, time::Duration};

use base64::{Engine, engine::general_purpose::STANDARD};
use ureq::Agent;

use crate::{Result, reject};

pub(crate) const CURRENT_USER_URL: &str = "https://api.bitbucket.org/2.0/user";

pub struct ReadClient {
    agent: Agent,
    authorization: String,
}

impl ReadClient {
    pub fn from_environment(token_var: &str, email_var: &str) -> Result<Self> {
        Self::from_credentials(token_var, email_var, None, None)
    }

    pub(crate) fn from_credentials(
        token_var: &str,
        email_var: &str,
        token_file: Option<&Path>,
        email: Option<String>,
    ) -> Result<Self> {
        let read_file = |path: &Path| -> Result<String> {
            let bytes = craft_factory_evidence::release_trace::read_evidence(path, 4098)
                .map_err(|_| "could not read the bounded Bitbucket credential file")?;
            let token = std::str::from_utf8(&bytes)
                .map_err(|_| "invalid read-only Bitbucket credential")?;
            Ok(token
                .strip_suffix("\r\n")
                .or_else(|| token.strip_suffix('\n'))
                .unwrap_or(token)
                .into())
        };
        let token = if let Some(path) = token_file {
            read_file(path)?
        } else {
            match std::env::var(token_var) {
                Ok(token) => token,
                Err(std::env::VarError::NotPresent) => {
                    let path =
                        crate::evidence_capture::credential_path(token_var).ok_or_else(|| {
                            format!("provide a read-only Bitbucket credential for {token_var}")
                        })?;
                    read_file(&path)?
                }
                Err(_) => return reject("invalid read-only Bitbucket credential"),
            }
        };
        if token.is_empty() || token.len() > 4096 || !token.bytes().all(|b| b.is_ascii_graphic()) {
            return reject("invalid read-only Bitbucket credential");
        }
        let authorization = match email.map_or_else(|| std::env::var(email_var), Ok) {
            Ok(email)
                if email.len() <= 320
                    && email.contains('@')
                    && !email.contains(':')
                    && email.bytes().all(|b| b.is_ascii_graphic()) =>
            {
                format!("Basic {}", STANDARD.encode(format!("{email}:{token}")))
            }
            Err(std::env::VarError::NotPresent) => format!("Bearer {token}"),
            _ => return reject("invalid Bitbucket account email"),
        };
        let agent = Agent::config_builder()
            .https_only(true)
            .max_redirects(0)
            .proxy(None)
            .timeout_global(Some(Duration::from_secs(8)))
            .build()
            .into();
        Ok(Self {
            agent,
            authorization,
        })
    }

    pub fn get(&self, url: &str, limit: u64) -> Result<Vec<u8>> {
        self.get_for(url, limit, Duration::from_secs(8))
    }

    pub(crate) fn get_for(&self, url: &str, limit: u64, timeout: Duration) -> Result<Vec<u8>> {
        const ORIGIN: &str = "https://api.bitbucket.org/2.0/repositories/";
        if !url.starts_with(ORIGIN) && url != CURRENT_USER_URL {
            return reject("unexpected Bitbucket evidence endpoint");
        }
        let mut response = self
            .agent
            .get(url)
            .config()
            .timeout_global(Some(timeout.min(Duration::from_secs(8))))
            .build()
            .header("Authorization", &self.authorization)
            .header("Accept", "application/json")
            .call()
            .map_err(|_| "could not read Bitbucket evidence")?;
        if response.status() != 200 {
            return reject("unexpected Bitbucket evidence response");
        }
        response
            .body_mut()
            .with_config()
            .limit(limit)
            .read_to_vec()
            .map_err(|_| "could not read the bounded Bitbucket evidence body".into())
    }
}
