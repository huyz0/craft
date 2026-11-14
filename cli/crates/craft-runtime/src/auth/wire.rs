use std::fmt;

use serde::de::{self, IgnoredAny, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use zeroize::Zeroizing;

pub(super) struct SecretText(pub(super) Zeroizing<String>);
impl<'de> Deserialize<'de> for SecretText {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self(Zeroizing::new(String::deserialize(deserializer)?)))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Provisioning {
    pub(super) client_id: SecretText,
    pub(super) client_secret: SecretText,
}

pub(super) struct TokenResponse {
    pub(super) access_token: SecretText,
    pub(super) token_type: SecretText,
    pub(super) expires_in: u64,
    pub(super) scope: Option<SecretText>,
}

impl<'de> Deserialize<'de> for TokenResponse {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct TokenVisitor;
        impl<'de> Visitor<'de> for TokenVisitor {
            type Value = TokenResponse;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a bounded OAuth token object")
            }
            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut seen: Vec<SecretText> = Vec::new();
                let (mut access_token, mut token_type, mut expires_in, mut scope) =
                    (None, None, None, None);
                while let Some(key) = map.next_key::<SecretText>()? {
                    if key.0.len() > 128
                        || seen.len() >= 64
                        || seen
                            .iter()
                            .any(|previous| previous.0.as_str() == key.0.as_str())
                    {
                        return Err(de::Error::custom("invalid or duplicate OAuth parameter"));
                    }
                    match key.0.as_str() {
                        "access_token" => access_token = Some(map.next_value()?),
                        "token_type" => token_type = Some(map.next_value()?),
                        "expires_in" => expires_in = Some(map.next_value()?),
                        "scope" => scope = Some(map.next_value()?),
                        // OAuth permits response extensions. Ignore them without retaining strings
                        // or interpreting a refresh-token extension as service renewal authority.
                        _ => {
                            map.next_value::<IgnoredAny>()?;
                        }
                    }
                    seen.push(key);
                }
                Ok(TokenResponse {
                    access_token: access_token
                        .ok_or_else(|| de::Error::missing_field("access_token"))?,
                    token_type: token_type.ok_or_else(|| de::Error::missing_field("token_type"))?,
                    expires_in: expires_in.ok_or_else(|| de::Error::missing_field("expires_in"))?,
                    scope,
                })
            }
        }
        deserializer.deserialize_map(TokenVisitor)
    }
}
