//! Android 16 PM delegates its pinned ART commands to the original ART owner.
use super::shell::Context;
use aim_binder_host::parcel::{EX_ILLEGAL_STATE, Exception};

#[path = "art_shell_commands.rs"]
mod commands;
fn supported(command: Option<&str>) -> bool {
    command.is_some_and(|command| commands::SUPPORTED.contains(&command))
}

pub fn run(context: &mut Context<'_>) -> Result<Option<i32>, Exception> {
    if !supported(context.command.args.first().map(String::as_str)) {
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_pinned_art_commands_enter_art_owner() {
        for command in commands::SUPPORTED {
            assert!(supported(Some(command)));
        }
        for command in [
            "install",
            "uninstall",
            "list",
            "path",
            "set-home-activity",
            "compile-extra",
            "",
            "ART",
        ] {
            assert!(
                !supported(Some(command)),
                "generic package command intercepted: {command}"
            );
        }
        assert!(!supported(None));
    }
}
