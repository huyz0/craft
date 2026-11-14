use std::collections::BTreeMap;

use craft_core::pagination::NativePosition;

#[test]
fn native_position_rejects_credentials_controls_and_excess_without_echoing_values()
-> Result<(), Box<dyn std::error::Error>> {
    for (name, value) in [
        ("authorization", "private-sentinel"),
        ("Access-Token", "private-sentinel"),
        ("client_secret", "private-sentinel"),
        ("cursor", "private-sentinel\n"),
        ("bad/name", "private-sentinel"),
        ("cursor", ""),
    ] {
        let error = NativePosition(BTreeMap::from([(name.into(), value.into())]))
            .validate()
            .err()
            .ok_or("Expected invalid position")?;
        assert!(!serde_json::to_string(&error)?.contains("private-sentinel"));
    }
    assert!(NativePosition(BTreeMap::new()).validate().is_err());
    assert!(
        NativePosition(BTreeMap::from([("cursor".into(), "x".repeat(16_385))]))
            .validate()
            .is_err()
    );
    NativePosition(BTreeMap::from([(
        "cursor".into(),
        "opaque+token/with=encoding%20文字".into(),
    )]))
    .validate()?;
    Ok(())
}
