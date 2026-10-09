//! Pinned create-user CLI delegates to the retained original UserManager owner.
use super::shell::Context;
use aim_binder_host::parcel::{EX_ILLEGAL_STATE, Exception};

fn supported(command: Option<&str>) -> bool { command == Some("create-user") }

pub fn run(context: &mut Context<'_>) -> Result<Option<i32>, Exception> {
    if !supported(context.command.command()) {
        return Ok(None);
    }
    let missing = |name| {
        Exception::new(
            EX_ILLEGAL_STATE,
            format!("User shell {name} descriptor unavailable"),
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
        .shell_create_user_command(context.uid, context.pid, input, output, error, args)
        .map(Some)
}


#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_create_user_enters_original_user_owner() {
        assert!(supported(Some("create-user")));
        for command in ["remove-user", "install", "list", "create-user-extra", "", "CREATE-USER"] {
            assert!(!supported(Some(command)));
        }
        assert!(!supported(None));
    }
}
