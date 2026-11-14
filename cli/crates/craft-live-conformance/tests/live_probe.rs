//! Prints the top-level keys and status of raw GETs named in CRAFT_LIVE_PROBE
//! (space separated paths). Values are not printed. Opt-in like the other live tests.

use craft_live_conformance::Live;

#[test]
#[ignore = "live tenant access; opt in with CRAFT_LIVE=1"]
fn live_probe() -> Result<(), Box<dyn std::error::Error>> {
    let live = Live::connect()?;
    let paths = std::env::var("CRAFT_LIVE_PROBE").unwrap_or_default();
    for path in paths.split_whitespace() {
        let exchange = live.get(path)?;
        let keys: Vec<String> = match &exchange.body {
            serde_json::Value::Object(map) => map
                .iter()
                .map(|(k, v)| match v {
                    serde_json::Value::Bool(b) => format!("{k}={b}"),
                    serde_json::Value::Number(n) => format!("{k}={n}"),
                    serde_json::Value::Array(a) => format!("{k}[{}]", a.len()),
                    _ => k.clone(),
                })
                .collect(),
            other => vec![format!(
                "<{}>",
                if other.is_array() { "array" } else { "scalar" }
            )],
        };
        eprintln!("{} -> {} {}", path, exchange.status, keys.join(" "));
    }
    Ok(())
}
