//! Pinned package user CLIs delegate to the retained original UserManager owner.
use super::shell::Context;
use aim_binder_host::parcel::{EX_ILLEGAL_STATE, Exception};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Owned {
    Create,
    List,
    Remove,
}
fn owned(arguments: &[String]) -> Option<Owned> {
    if arguments
        .first()
        .is_some_and(|command| command == "create-user")
    {
        Some(Owned::Create)
    } else if arguments
        .first()
        .is_some_and(|command| command == "remove-user")
    {
        Some(Owned::Remove)
    } else if arguments.first().is_some_and(|command| command == "list")
        && arguments.get(1).is_some_and(|noun| noun == "users")
    {
        Some(Owned::List)
    } else {
        None
    }
}

pub fn run(context: &mut Context<'_>) -> Result<Option<i32>, Exception> {
    let Some(owned) = owned(&context.command.args) else {
        return Ok(None);
    };
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
    let system = context.system()?;
    match owned {
        Owned::Create => system.shell_create_user_command(
            context.uid,
            context.pid,
            input,
            output,
            error,
            context.command.args.clone(),
        ),
        Owned::List => {
            system.shell_list_users_command(context.uid, context.pid, input, output, error)
        }
        Owned::Remove => system.shell_remove_user_command(
            context.uid,
            context.pid,
            input,
            output,
            error,
            context.command.args.clone(),
        ),
    }
    .map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_owned_user_commands_enter_original_user_owner() {
        assert_eq!(
            owned(&["create-user".into(), "fixture".into()]),
            Some(Owned::Create)
        );
        assert_eq!(owned(&["list".into(), "users".into()]), Some(Owned::List));
        assert_eq!(
            owned(&["list".into(), "users".into(), "ignored".into()]),
            Some(Owned::List)
        );
        assert_eq!(
            owned(&["remove-user".into(), "10".into()]),
            Some(Owned::Remove)
        );
        for arguments in [
            vec!["install".into()],
            vec!["list".into()],
            vec!["list".into(), "packages".into()],
            vec!["create-user-extra".into()],
            vec![],
            vec!["CREATE-USER".into()],
        ] {
            assert_eq!(owned(&arguments), None);
        }
    }
}
