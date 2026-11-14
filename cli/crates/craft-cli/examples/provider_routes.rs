//! Development-only native route compatibility check. It never registers or invokes a provider.

use craft_core::execution::{HttpMethod, NativeRoute, RouteSegment};
use serde::Deserialize;
use serde_json::{Value, json};
use std::io::Read;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RouteShape {
    path: Vec<RouteSegment>,
    query_parameters: Vec<String>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../openspec/changes/build-cloud-cli/provider-operations.json"
    );
    let mut source = Vec::new();
    std::fs::File::open(path)?
        .take(1_048_577)
        .read_to_end(&mut source)?;
    if source.len() > 1_048_576 {
        return Err("Development matrix exceeds its bound".into());
    }
    let matrix: Value = serde_json::from_slice(&source)?;
    let commands = matrix["commands"].as_array().ok_or("Missing commands")?;
    let mut count = 0;
    for command in commands {
        if command["runtime_available"] != false {
            return Err("Development matrix cannot enable a runtime capability".into());
        }
        for variant in command["variants"].as_array().ok_or("Missing variants")? {
            let shape: RouteShape = serde_json::from_value(variant["route_shape"].clone())?;
            let method: HttpMethod = serde_json::from_value(json!(
                variant["method"]
                    .as_str()
                    .ok_or("Missing method")?
                    .to_ascii_lowercase()
            ))?;
            let route = NativeRoute {
                repeated_query_parameters: Vec::new(),
                id: variant["operation_id"]
                    .as_str()
                    .ok_or("Missing operation")?
                    .into(),
                method,
                path: shape.path,
                query_parameters: shape.query_parameters,
                continuation_parameters: Vec::new(),
                // The minimum bound checks syntax only; adapters must choose their actual budgets.
                response_max_bytes: 512,
            };
            route.validate()?;
            count += 1;
        }
    }
    println!(
        "{}",
        json!({"ok":true,"native_route_shapes":count,"runtime_available":false})
    );
    Ok(())
}
