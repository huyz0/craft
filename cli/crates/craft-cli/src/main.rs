use craft_cli::{args, catalog, dispatch, output, profiles};

use std::io::{self, Write};
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use clap::{CommandFactory, FromArgMatches, error::ErrorKind};
use craft_core::contracts::Envelope;
use craft_core::error::{CraftError, ErrorCode};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

enum CommandOutput {
    Data(Box<dispatch::CommandResult>),
    HumanHelp,
}

fn request_id() -> String {
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis());
    format!(
        "req-{:x}-{:x}-{:x}",
        std::process::id(),
        time,
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}

fn write_result(
    envelope: Envelope,
    maximum: usize,
    policy: output::OutputPolicy,
) -> io::Result<u8> {
    let result = output::encode(envelope, maximum, policy).map_err(io::Error::other)?;
    io::stdout().lock().write_all(&result.bytes)?;
    Ok(result.exit)
}

fn parse_arguments() -> Result<(args::Cli, bool), clap::Error> {
    let mut command = args::Cli::command();
    let matches = command.try_get_matches_from_mut(std::env::args_os())?;
    Ok((
        args::Cli::from_arg_matches(&matches)?,
        args::has_action_flags(&command, &matches),
    ))
}

fn run() -> io::Result<u8> {
    let (arguments, action_flags) = match parse_arguments() {
        Ok(arguments) => arguments,
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) =>
        {
            io::stdout()
                .lock()
                .write_all(error.to_string().as_bytes())?;
            return Ok(0);
        }
        Err(error) => {
            let code = if matches!(error.kind(), ErrorKind::InvalidSubcommand) {
                ErrorCode::CommandUnknown
            } else {
                ErrorCode::InputInvalid
            };
            let envelope = Envelope::failure(
                "cli",
                request_id(),
                CraftError::new(
                    code,
                    "Invalid command arguments; use help --json or selected-command --help.",
                ),
            );
            write_result(envelope, 16_384, output::OutputPolicy::Bounded)?;
            return Ok(code.exit_code());
        }
    };
    if arguments.input.is_none()
        && (arguments.action.is_none()
            || matches!(arguments.action.as_ref(), Some(args::Action::Help(help)) if !help.json))
    {
        args::Cli::command().write_long_help(&mut io::stdout().lock())?;
        return Ok(0);
    }
    let Some(action) = arguments.action else {
        let error = CraftError::field("/input", "input_requires_a_command_path");
        write_result(
            Envelope::failure("cli", request_id(), error),
            arguments.max_bytes as usize,
            output::OutputPolicy::Bounded,
        )?;
        return Ok(10);
    };
    let command = dispatch::command_id(&action);
    let id = request_id();
    let stdin_consumed = arguments.input.as_deref() == Some("-");
    let result =
        craft_runtime::control::install_control(arguments.timeout_ms).and_then(|control| {
            catalog::registry().and_then(|registry| {
                control.check()?;
                let action = if let Some(file) = arguments.input {
                    if action_flags {
                        return Err(CraftError::field(
                            "/input",
                            "input_and_action_flags_are_exclusive",
                        ));
                    }
                    dispatch::load_input(&registry, action, &file, &control)?
                } else {
                    action
                };
                control.check()?;
                if matches!(&action, args::Action::Help(help) if !help.json) {
                    return Ok(CommandOutput::HumanHelp);
                }
                dispatch::execute(
                    &registry,
                    action,
                    &arguments.fields,
                    &control,
                    &profiles::InvocationOptions {
                        state_root: arguments.state_root.as_deref(),
                        profile: arguments.profile.as_deref(),
                        request_id: &id,
                        max_bytes: arguments.max_bytes as usize,
                    },
                    stdin_consumed,
                )
                .map(|result| CommandOutput::Data(Box::new(result)))
            })
        });
    let result = match result {
        Ok(CommandOutput::HumanHelp) => {
            args::Cli::command().write_long_help(&mut io::stdout().lock())?;
            return Ok(0);
        }
        Ok(CommandOutput::Data(output)) => *output,
        Err(error) => dispatch::CommandResult {
            envelope: Envelope::failure(command, id, error),
            output_policy: output::OutputPolicy::Bounded,
        },
    };
    write_result(
        result.envelope,
        arguments.max_bytes as usize,
        result.output_policy,
    )
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => ExitCode::from(code),
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
    }
}
