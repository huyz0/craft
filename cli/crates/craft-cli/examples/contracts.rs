use std::io::{self, Write};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let contracts = craft_cli::contracts::snapshot()?;
    let mut output = io::stdout().lock();
    serde_json::to_writer_pretty(&mut output, &contracts)?;
    output.write_all(b"\n")?;
    Ok(())
}
