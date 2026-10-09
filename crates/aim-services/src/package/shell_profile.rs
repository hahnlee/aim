//! Android 16 snapshot-profile executes the original ART Service shell owner.
use super::shell::Context;
use aim_binder_host::parcel::{EX_ILLEGAL_STATE, Exception};

pub fn run(context: &mut Context<'_>) -> Result<Option<i32>, Exception> {
    if context.command.args.first().map(String::as_str) != Some("snapshot-profile") {
        return Ok(None);
    }
    let missing = |name| {
        Exception::new(
            EX_ILLEGAL_STATE,
            format!("ART shell {name} descriptor unavailable"),
        )
    };
    let input = context
        .command
        .input()
        .ok_or_else(|| missing("input"))?
        .try_clone()
        .map_err(|error| Exception::new(EX_ILLEGAL_STATE, error.to_string()))?;
    let output = context
        .command
        .output()
        .ok_or_else(|| missing("output"))?
        .try_clone()
        .map_err(|error| Exception::new(EX_ILLEGAL_STATE, error.to_string()))?;
    let error = context
        .command
        .error()
        .ok_or_else(|| missing("error"))?
        .try_clone()
        .map_err(|error| Exception::new(EX_ILLEGAL_STATE, error.to_string()))?;
    let args = context.command.args.clone();
    context
        .system()?
        .shell_art_command(context.uid, context.pid, input, output, error, args)
        .map(Some)
}
