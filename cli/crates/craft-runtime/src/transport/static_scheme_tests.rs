use super::*;
use craft_core::contracts::TenantRef;

fn secret(text: &str) -> Result<SecretBytes, Box<dyn std::error::Error>> {
    Ok(SecretBytes::new(text.as_bytes().to_vec())?)
}

fn tenant(provider: &str, id: &str) -> TenantRef {
    TenantRef {
        provider: provider.into(),
        deployment: "cloud".into(),
        tenant: id.into(),
    }
}

#[test]
fn trello_secret_becomes_header_fields_not_url_parts() -> Result<(), Box<dyn std::error::Error>> {
    let header = trello_header(&secret("abc123:tok456")?)?;
    assert_eq!(
        header.to_str()?,
        "OAuth oauth_consumer_key=\"abc123\", oauth_token=\"tok456\""
    );
    assert!(header.is_sensitive());
    Ok(())
}

#[test]
fn static_secrets_that_could_break_a_header_field_are_refused()
-> Result<(), Box<dyn std::error::Error>> {
    for bad in [
        "nocolon",
        ":token",
        "key:",
        "k\"y:t",
        "k,y:t",
        "key:to:ken",
        "ke y:t",
    ] {
        assert!(trello_header(&secret(bad)?).is_err(), "{bad}");
    }
    Ok(())
}

#[test]
fn basic_header_matches_the_standard_encoding() -> Result<(), Box<dyn std::error::Error>> {
    // RFC 7617 example plus padding cases.
    assert_eq!(
        base64(b"Aladdin:open sesame"),
        "QWxhZGRpbjpvcGVuIHNlc2FtZQ=="
    );
    assert_eq!(base64(b"a"), "YQ==");
    assert_eq!(base64(b"ab"), "YWI=");
    assert_eq!(base64(b"abc"), "YWJj");
    let header = basic_header(&secret("agent@example.test:apitoken")?)?;
    assert_eq!(
        header.to_str()?,
        "Basic YWdlbnRAZXhhbXBsZS50ZXN0OmFwaXRva2Vu"
    );
    Ok(())
}

#[test]
fn fixed_hosts_ignore_everything_but_the_provider() -> Result<(), Box<dyn std::error::Error>> {
    let trello = routing::Endpoint::for_tenant(&tenant("trello", &"a".repeat(24)))?;
    assert_eq!(trello.prefix.as_str(), "https://api.trello.com/1/");
    let bitbucket = routing::Endpoint::for_tenant(&tenant("bitbucket", "team-space"))?;
    assert_eq!(bitbucket.prefix.as_str(), "https://api.bitbucket.org/2.0/");
    for bad in ["", "Team", "-x", "a/b", "..", "a b", "a?b"] {
        assert!(
            routing::Endpoint::for_tenant(&tenant("bitbucket", bad)).is_err(),
            "{bad}"
        );
    }
    assert!(routing::Endpoint::for_tenant(&tenant("trello", "short")).is_err());
    Ok(())
}

#[test]
fn identity_probe_urls_use_the_fixed_hosts() -> Result<(), Box<dyn std::error::Error>> {
    let url = product_identity_url(&tenant("trello", &"0".repeat(24)))?;
    assert_eq!(url.as_str(), "https://api.trello.com/1/members/me");
    let url = product_identity_url(&tenant("bitbucket", "team-space"))?;
    assert_eq!(
        url.as_str(),
        "https://api.bitbucket.org/2.0/workspaces/team-space"
    );
    Ok(())
}

#[test]
fn a_scheme_cannot_be_bound_to_another_providers_tenant() -> Result<(), Box<dyn std::error::Error>>
{
    let grant = GrantRef {
        id: "static-grant".into(),
    };
    let result = HostSession::with_scheme(
        tenant("bitbucket", "team-space"),
        grant,
        secret("abc123:tok456")?,
        CredentialScheme::TrelloKeyToken,
    );
    assert!(result.is_err());
    Ok(())
}
