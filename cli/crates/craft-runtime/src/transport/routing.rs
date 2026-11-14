use std::collections::BTreeMap;

use craft_core::contracts::TenantRef;
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::{HttpMethod, NativeCall, NativeRoute, RouteSegment};
use url::Url;

const MAX_URL_BYTES: usize = 32_768;

/// A host-supplied validation constraint for project-search links. Never an HTTP destination.
#[derive(Clone)]
pub struct JiraProjectSite {
    origin: Url,
}

/// An exact field-search link constraint. It never changes the authenticated endpoint.
#[derive(Clone)]
pub struct JiraFieldSite {
    origin: Url,
}
impl JiraFieldSite {
    pub fn parse(value: &str) -> Result<Self, CraftError> {
        Ok(Self {
            origin: JiraProjectSite::parse(value)?.origin,
        })
    }
    pub fn as_str(&self) -> &str {
        self.origin.as_str()
    }
    pub(super) fn permits(route: &NativeRoute) -> bool {
        route.id == "jira-field-search"
            && matches!(route.method, HttpMethod::Get)
            && route
                .path
                .iter()
                .map(|part| match part {
                    RouteSegment::Literal(value) => value.as_str(),
                    _ => "",
                })
                .eq(["rest", "api", "3", "field", "search"])
            && route.continuation_parameters == ["startAt"]
            && route
                .repeated_query_parameters
                .iter()
                .map(String::as_str)
                .collect::<std::collections::BTreeSet<_>>()
                == ["id", "type", "projectIds"].into_iter().collect()
            && route
                .query_parameters
                .iter()
                .map(String::as_str)
                .collect::<std::collections::BTreeSet<_>>()
                == [
                    "startAt",
                    "maxResults",
                    "type",
                    "id",
                    "query",
                    "orderBy",
                    "expand",
                    "projectIds",
                ]
                .into_iter()
                .collect()
    }
    pub(super) fn normalize(
        &self,
        route: &NativeRoute,
        call: &NativeCall,
        link: &str,
    ) -> Result<NativeCall, CraftError> {
        if !Self::permits(route) {
            return Err(denied());
        }
        Endpoint {
            prefix: self.origin.clone(),
        }
        .normalize_next(route, call, link)
    }
}

impl JiraProjectSite {
    pub fn parse(value: &str) -> Result<Self, CraftError> {
        let invalid = || {
            CraftError::field(
                "/continuation_site",
                "expected_plain_jira_cloud_https_origin",
            )
        };
        if value.len() > 255
            || value.trim() != value
            || value.contains(['\\', '?', '#'])
            || value.chars().any(char::is_control)
            || value
                .split_once("://")
                .and_then(|(_, rest)| rest.split_once('/'))
                .is_some_and(|(_, path)| !path.is_empty())
        {
            return Err(invalid());
        }
        let origin = Url::parse(value).map_err(|_| invalid())?;
        let name = origin
            .host_str()
            .and_then(|host| host.strip_suffix(".atlassian.net"))
            .ok_or_else(invalid)?;
        if origin.scheme() != "https" || !origin.username().is_empty() || origin.password().is_some()
            || origin.port().is_some() || origin.path() != "/" || name.is_empty() || name.len() > 63
            || !name.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            || name.starts_with('-') || name.ends_with('-')
            // The URL parser normalizes explicit default ports; the input must still be an origin without one.
            || value.split("://").nth(1).is_none_or(|authority| authority.trim_end_matches('/').contains(':'))
        {
            return Err(invalid());
        }
        Ok(Self { origin })
    }

    pub fn as_str(&self) -> &str {
        self.origin.as_str()
    }

    pub(super) fn permits(route: &NativeRoute) -> bool {
        route.id == "jira-project-search"
            && matches!(route.method, HttpMethod::Get)
            && route
                .path
                .iter()
                .map(|part| match part {
                    RouteSegment::Literal(value) => value.as_str(),
                    _ => "",
                })
                .eq(["rest", "api", "3", "project", "search"])
            && route.continuation_parameters == ["startAt"]
            && route.repeated_query_parameters.is_empty()
            && route
                .query_parameters
                .iter()
                .map(String::as_str)
                .collect::<std::collections::BTreeSet<_>>()
                == ["startAt", "maxResults", "orderBy", "action", "query"]
                    .into_iter()
                    .collect()
    }

    pub(super) fn normalize(
        &self,
        route: &NativeRoute,
        call: &NativeCall,
        link: &str,
    ) -> Result<NativeCall, CraftError> {
        if !Self::permits(route) {
            return Err(denied());
        }
        Endpoint {
            prefix: self.origin.clone(),
        }
        .normalize_next(route, call, link)
    }
}

pub(super) struct Endpoint {
    pub(super) prefix: Url,
}

pub(super) fn denied() -> CraftError {
    CraftError::new(
        ErrorCode::PolicyDenied,
        "HTTP routing or continuation does not match the trusted operation.",
    )
}

fn cloud_id(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

fn query_component(raw: &str) -> Result<String, CraftError> {
    let bytes = raw.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'%'
            && (index + 2 >= bytes.len()
                || !bytes[index + 1].is_ascii_hexdigit()
                || !bytes[index + 2].is_ascii_hexdigit())
        {
            return Err(denied());
        }
    }
    percent_encoding::percent_decode_str(&raw.replace('+', " "))
        .decode_utf8()
        .map(|value| value.into_owned())
        .map_err(|_| denied())
}

impl Endpoint {
    pub(super) fn atlassian(tenant: &TenantRef) -> Result<Self, CraftError> {
        tenant.validate()?;
        if tenant.deployment != "cloud"
            || !matches!(tenant.provider.as_str(), "jira" | "confluence")
        {
            return Err(CraftError::new(
                ErrorCode::UnsupportedCapability,
                "No trusted HTTP template exists for this provider/deployment.",
            ));
        }
        if !cloud_id(&tenant.tenant) {
            return Err(CraftError::field("/target/tenant", "expected_cloud_uuid"));
        }
        let mut prefix = Url::parse("https://api.atlassian.com/").map_err(|_| denied())?;
        prefix.path_segments_mut().map_err(|_| denied())?.extend([
            "ex",
            &tenant.provider,
            &tenant.tenant,
            "",
        ]);
        Ok(Self { prefix })
    }

    pub(super) fn route_url(
        &self,
        route: &NativeRoute,
        call: &NativeCall,
    ) -> Result<Url, CraftError> {
        route.validate_call(call)?;
        let mut url = self.prefix.clone();
        {
            let mut path = url.path_segments_mut().map_err(|_| denied())?;
            path.pop_if_empty();
            for segment in &route.path {
                match segment {
                    RouteSegment::Literal(value) => {
                        path.push(value);
                    }
                    RouteSegment::Parameter(name) => {
                        let value = call.path_parameters.get(name).ok_or_else(denied)?;
                        // IDs cannot become separators or dot traversal after upstream decoding.
                        if matches!(value.as_str(), "." | "..") || value.contains(['/', '\\']) {
                            return Err(denied());
                        }
                        path.push(value);
                    }
                }
            }
        }
        if !call.query.is_empty() || !call.repeated_query.is_empty() {
            let mut query = url.query_pairs_mut();
            for (name, value) in &call.query {
                query.append_pair(name, value);
            }
            for (name, values) in &call.repeated_query {
                for value in values {
                    query.append_pair(name, value);
                }
            }
        }
        if url.as_str().len() > MAX_URL_BYTES {
            return Err(CraftError::field("/query", "native_url_limit_exceeded"));
        }
        Ok(url)
    }

    pub(super) fn normalize_next(
        &self,
        route: &NativeRoute,
        call: &NativeCall,
        link: &str,
    ) -> Result<NativeCall, CraftError> {
        if !matches!(route.method, HttpMethod::Get)
            || call.body.is_some()
            || link.is_empty()
            || link.len() > MAX_URL_BYTES
            || link.chars().any(char::is_control)
            || link.contains('\\')
        {
            return Err(denied());
        }
        let current = self.route_url(route, call)?;
        let next = current.join(link).map_err(|_| denied())?;
        if next.origin() != current.origin()
            || next.path() != current.path()
            || !next.username().is_empty()
            || next.password().is_some()
            || next.fragment().is_some()
        {
            return Err(denied());
        }
        let mut query = BTreeMap::new();
        let mut repeated: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut pairs = 0usize;
        for pair in next
            .query()
            .unwrap_or("")
            .split('&')
            .filter(|pair| !pair.is_empty())
        {
            pairs += 1;
            if pairs > 256 {
                return Err(denied());
            }
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            let name = query_component(name)?;
            let value = query_component(value)?;
            if route.repeated_query_parameters.contains(&name) {
                let values = repeated.entry(name).or_default();
                if values.len() >= 100 {
                    return Err(denied());
                }
                values.push(value);
            } else if query.insert(name, value).is_some() {
                return Err(denied());
            }
        }
        let mut normalized = call.clone();
        normalized
            .query
            .retain(|name, _| !route.continuation_parameters.contains(name));
        for (name, value) in query {
            if !route.query_parameters.contains(&name) {
                return Err(denied());
            }
            if !route.continuation_parameters.contains(&name)
                && call.query.get(&name) != Some(&value)
            {
                return Err(denied());
            }
            normalized.query.insert(name, value);
        }
        // A next link can omit bound filters, but it cannot replace or partially repeat an array.
        for (name, values) in repeated {
            if call.repeated_query.get(&name) != Some(&values) {
                return Err(denied());
            }
        }
        route.validate_call(&normalized)?;
        self.route_url(route, &normalized)?;
        Ok(normalized)
    }
}
