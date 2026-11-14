use std::process::Command;

#[test]
fn identifies_the_distribution_and_version() -> Result<(), Box<dyn std::error::Error>> {
    let output = Command::new(env!("CARGO_BIN_EXE_craft"))
        .arg("--version")
        .output()?;
    assert!(output.status.success());
    let identity = String::from_utf8(output.stdout)?;
    assert!(identity.contains(craft_core::PRODUCT));
    assert!(identity.contains(craft_core::VERSION));
    assert!(output.stderr.is_empty());
    Ok(())
}
