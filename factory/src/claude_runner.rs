use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::process::Command;

use crate::{
    Result,
    order::{AgentAdapter, ExecutionProfile},
    reject,
};

pub fn api_key() -> Result<String> {
    let key = std::env::var("ANTHROPIC_API_KEY")
        .map_err(|_| "the Claude adapter requires ANTHROPIC_API_KEY")?;
    if key.is_empty() || key.len() > 4096 || !key.bytes().all(|byte| byte.is_ascii_graphic()) {
        return reject("the Claude API key has an invalid format");
    }
    Ok(key)
}

pub fn validate_executable(executable: &Path) -> Result<()> {
    let mut magic = [0; 4];
    File::open(executable)?.read_exact(&mut magic)?;
    if &magic != b"\x7fELF" {
        return reject("the Claude adapter requires the native Linux executable");
    }
    Ok(())
}

pub fn command(
    executable: &Path,
    bubblewrap: &Path,
    checkout: &Path,
    result_schema: &Path,
    profile: Option<&ExecutionProfile>,
) -> Result<Command> {
    if !cfg!(target_os = "linux")
        || !executable.is_absolute()
        || !bubblewrap.is_absolute()
        || !checkout.is_absolute()
    {
        return reject("the Claude adapter requires Linux and absolute sandbox inputs");
    }
    validate_executable(executable)?;
    let schema = std::fs::read_to_string(result_schema)?;
    let mut command = Command::new(bubblewrap);
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", "/tmp/home")
        .env("TMPDIR", "/tmp")
        .env("ANTHROPIC_API_KEY", api_key()?)
        .args([
            "--unshare-all",
            "--share-net",
            "--die-with-parent",
            "--cap-drop",
            "ALL",
            "--ro-bind",
            "/usr",
            "/usr",
            "--ro-bind-try",
            "/bin",
            "/bin",
            "--ro-bind-try",
            "/lib",
            "/lib",
            "--ro-bind-try",
            "/lib64",
            "/lib64",
            "--ro-bind-try",
            "/etc/ssl",
            "/etc/ssl",
            "--ro-bind-try",
            "/etc/resolv.conf",
            "/etc/resolv.conf",
            "--ro-bind-try",
            "/etc/hosts",
            "/etc/hosts",
            "--ro-bind-try",
            "/etc/nsswitch.conf",
            "/etc/nsswitch.conf",
            "--ro-bind-try",
            "/etc/claude-code",
            "/etc/claude-code",
            "--ro-bind-try",
            "/etc/passwd",
            "/etc/passwd",
            "--ro-bind-try",
            "/etc/group",
            "/etc/group",
            "--dev",
            "/dev",
            "--proc",
            "/proc",
            "--tmpfs",
            "/tmp",
            "--dir",
            "/tmp/home",
            "--dir",
            "/runner",
            "--ro-bind",
        ])
        .arg(executable)
        .args(["/runner/claude", "--bind"])
        .arg(checkout)
        .args(["/work", "--ro-bind"])
        .arg(checkout.join(".git"))
        .args([
            "/work/.git",
            "--chdir",
            "/work",
            "--",
            "/runner/claude",
            "--restricted",
            "--bare",
            "--print",
            "--no-session-persistence",
            "--no-chrome",
            "--output-format",
            "json",
            "--permission-mode",
            "dontAsk",
            "--tools",
            "Read,Edit,Write,Glob,Grep",
            "--allowedTools",
            "Read,Edit,Write,Glob,Grep",
            "--disallowedTools",
            "mcp__*",
            "--strict-mcp-config",
            "--mcp-config",
            "{\"mcpServers\":{}}",
            "--settings",
            "{\"disableClaudeAiConnectors\":true}",
            "--json-schema",
        ])
        .arg(schema);
    if let Some(profile) = profile {
        if !profile.valid() || !profile.supports_adapter(AgentAdapter::Claude) {
            return reject("invalid Claude execution profile in runner launch permit");
        }
        command.args([
            "--model",
            &profile.model,
            "--effort",
            profile.reasoning_effort.as_str(),
        ]);
    }
    Ok(command)
}
