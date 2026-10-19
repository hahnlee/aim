//! `settings`: SettingsProvider's `SettingsService` at `android-16.0.0_r1`,
//! which runs the shell's `settings` command (`MyShellCommand`): `get`,
//! `put`, `delete`, `list` and `reset` of a user's system, secure and
//! global settings, each a `call` of the provider as the caller. Its dump:
//! #1585.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::HashMap;
use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, EX_ILLEGAL_ARGUMENT, EX_NULL_POINTER, EX_SECURITY, Exception, Parcel, UNKNOWN_TRANSACTION};

use super::settings_provider::{Caller, SettingsProvider};
use super::{AppHost, reply};
use crate::bundle::Value;
use crate::shell::{SHELL_COMMAND_TRANSACTION, ShellCommand};

/// `IBinder.DUMP_TRANSACTION`.
const DUMP_TRANSACTION: u32 = u32::from_be_bytes(*b"_DMP");
/// `UserHandle.USER_NULL`, `USER_ALL`, `USER_CURRENT`.
const USER_NULL: i32 = -10_000;
const USER_ALL: i32 = -1;
const USER_CURRENT: i32 = -2;
/// `Process.ROOT_UID`, `SHELL_UID`.
const ROOT_UID: i32 = 0;
const SHELL_UID: i32 = 2000;

pub struct SettingsService {
    host: Arc<AppHost>,
    provider: Arc<SettingsProvider>,
}

impl SettingsService {
    pub fn new(host: Arc<AppHost>, provider: Arc<SettingsProvider>) -> SettingsService {
        SettingsService { host, provider }
    }
}

impl Service for SettingsService {
    fn descriptor(&self) -> &str {
        ""
    }

    fn has_descriptor(&self) -> bool {
        false
    }

    fn accepts_fds(&self) -> bool {
        true
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        match call.code {
            SHELL_COMMAND_TRANSACTION => {
                let command = ShellCommand::read(&self.host.process, &mut call.data).map_err(|_| BAD_VALUE)?;
                if command.has_output() {
                    let uid = call.sender_euid as i32;
                    let mut command = command;
                    let result = run(&self.provider, uid, &mut command);
                    command.finish(result);
                }
                let mut p = Parcel::new();
                p.write_no_exception();
                Ok(p)
            }
            DUMP_TRANSACTION => reply(Err(self.host.unimplemented_named("settings", "dump (#1585)"))),
            _ => Err(UNKNOWN_TRANSACTION),
        }
    }
}

/// The verbs (`CommandVerb`).
#[derive(Clone, Copy, PartialEq)]
enum Verb {
    Get,
    Put,
    Delete,
    List,
    Reset,
}

/// What a command line asks for.
struct Command {
    user: i32,
    verb: Option<Verb>,
    table: Option<String>,
    key: Option<String>,
    value: Option<String>,
    tag: Option<String>,
    reset_mode: i32,
    make_default: bool,
    overrideable_by_restore: bool,
}

/// An exception `exec` prints.
fn java_exception(e: &Exception) -> String {
    let class = match e.code {
        EX_SECURITY => "java.lang.SecurityException",
        EX_ILLEGAL_ARGUMENT => "java.lang.IllegalArgumentException",
        EX_NULL_POINTER => "java.lang.NullPointerException",
        aim_binder_host::parcel::EX_UNSUPPORTED_OPERATION => "java.lang.UnsupportedOperationException",
        _ => "java.lang.IllegalStateException",
    };
    format!("{class}: {}", e.message)
}

/// `UserHandle.parseUserArg`.
fn parse_user_arg(arg: &str) -> Result<i32, String> {
    match arg {
        "all" => Ok(USER_ALL),
        "current" | "cur" => Ok(USER_CURRENT),
        _ => arg.parse().map_err(|_| format!("java.lang.IllegalArgumentException: Bad user number: {arg}")),
    }
}

/// `exec` of `onCommand`: its result, an exception printed as -1.
fn run(provider: &SettingsProvider, uid: i32, c: &mut ShellCommand) -> i32 {
    match on_command(provider, uid, c) {
        Ok(result) => result,
        Err(e) => {
            c.exception(&e);
            -1
        }
    }
}

/// `parseOptionalPutArgument`: the tag, then `default`, then
/// `overrideableByRestore`, each once.
fn optional_put_argument(command: &mut Command, arg: &str) -> bool {
    match arg {
        "overrideableByRestore" if command.overrideable_by_restore => false,
        "overrideableByRestore" => {
            command.overrideable_by_restore = true;
            true
        }
        "default" if command.make_default || command.overrideable_by_restore => false,
        "default" => {
            command.make_default = true;
            true
        }
        _ if command.make_default || command.overrideable_by_restore || command.tag.is_some() => false,
        _ => {
            command.tag = Some(arg.to_string());
            true
        }
    }
}

/// `MyShellCommand.onCommand`: the command line read, then the call it
/// makes.
fn on_command(provider: &SettingsProvider, uid: i32, c: &mut ShellCommand) -> Result<i32, String> {
    let Some(cmd) = c.command().map(str::to_string).filter(|cmd| !matches!(cmd.as_str(), "help" | "-h")) else {
        help(c);
        return Ok(-1);
    };
    let mut command = Command {
        user: USER_NULL,
        verb: None,
        table: None,
        key: None,
        value: None,
        tag: None,
        reset_mode: -1,
        make_default: false,
        overrideable_by_restore: false,
    };
    let mut valid = false;
    let mut arg = Some(cmd);
    while let Some(a) = arg {
        if a == "--user" {
            if command.user != USER_NULL {
                c.eprintln("Invalid user: --user specified more than once");
                break;
            }
            command.user = parse_user_arg(&c.next_arg_required()?)?;
            if command.user == USER_ALL {
                c.eprintln("Invalid user: all");
                return Ok(-1);
            }
        } else if command.verb.is_none() {
            command.verb = Some(match a.to_ascii_lowercase().as_str() {
                "get" => Verb::Get,
                "put" => Verb::Put,
                "delete" => Verb::Delete,
                "list" => Verb::List,
                "reset" => Verb::Reset,
                _ => {
                    c.eprintln(&format!("Invalid command: {a}"));
                    return Ok(-1);
                }
            });
        } else if command.table.is_none() {
            let table = a.to_ascii_lowercase();
            if !matches!(table.as_str(), "system" | "secure" | "global") {
                c.eprintln(&format!("Invalid namespace '{a}'"));
                return Ok(-1);
            }
            command.table = Some(table);
            if command.verb == Some(Verb::List) {
                valid = true;
                break;
            }
        } else if command.verb == Some(Verb::Reset) {
            // `Settings.RESET_MODE_*`.
            command.reset_mode = match a.to_ascii_lowercase().as_str() {
                "untrusted_defaults" => 2,
                "untrusted_clear" => 3,
                "trusted_defaults" => 4,
                _ => 1,
            };
            // A package's defaults: the package (`mPackageName`, which no
            // call passes) and a tag.
            if command.reset_mode == 1 {
                if c.peek_next_arg().is_some() {
                    command.tag = c.next_arg();
                    if c.peek_next_arg().is_some() {
                        c.eprintln("Too many arguments");
                        return Ok(-1);
                    }
                }
                valid = true;
                break;
            }
            if c.peek_next_arg().is_some() {
                c.eprintln("Too many arguments");
                return Ok(-1);
            }
            valid = true;
        } else if matches!(command.verb, Some(Verb::Get | Verb::Delete)) {
            command.key = Some(a);
            if c.peek_next_arg().is_some() {
                c.eprintln("Too many arguments");
                return Ok(-1);
            }
            valid = true;
            break;
        } else if command.key.is_none() {
            command.key = Some(a);
        } else if command.value.is_none() {
            command.value = Some(a);
            valid = true;
        } else {
            valid = optional_put_argument(&mut command, &a);
        }
        arg = c.next_arg();
    }
    if !valid {
        c.eprintln("Bad arguments");
        return Ok(-1);
    }
    // The current user is the app host's one; it has no other.
    if matches!(command.user, USER_NULL | USER_CURRENT) {
        command.user = 0;
    }
    if command.user != 0 {
        c.eprintln(&format!("Invalid user: {}", command.user));
        return Ok(-1);
    }
    let caller = Caller {
        uid,
        package: match uid {
            ROOT_UID => Some("root".to_string()),
            SHELL_UID => Some("com.android.shell".to_string()),
            _ => None,
        },
    };
    let table = command.table.clone().unwrap_or_default();
    let mut args: HashMap<String, Value> = HashMap::from([("_user".to_string(), Value::Int(command.user))]);
    let call = |method: &str, key: Option<&str>, args: &HashMap<String, Value>| {
        provider.call(&caller, &format!("{method}_{table}"), key, args, None).map_err(|e| java_exception(&e))
    };
    match command.verb {
        Some(Verb::Get) => {
            // `Bundle.getPairValue`.
            let result = call("GET", command.key.as_deref(), &args)?;
            let value = match result.as_deref() {
                Some([(_, Value::String(Some(value))), ..]) => value.clone(),
                _ => "null".to_string(),
            };
            c.println(&value);
        }
        Some(Verb::Put) => {
            if table == "system" && command.make_default {
                c.print("Ignored makeDefault - doesn't apply to system settings");
                command.make_default = false;
            }
            args.insert("value".into(), Value::String(command.value.clone()));
            if let Some(tag) = &command.tag {
                args.insert("_tag".into(), Value::String(Some(tag.clone())));
            }
            if command.make_default {
                args.insert("_make_default".into(), Value::Bool(true));
            }
            if command.overrideable_by_restore {
                args.insert("_overrideable_by_restore".into(), Value::Bool(true));
            }
            call("PUT", command.key.as_deref(), &args)?;
        }
        Some(Verb::Delete) => {
            let deleted = match call("DELETE", command.key.as_deref(), &args)?.as_deref() {
                Some([(_, Value::Int(rows))]) => *rows,
                _ => 0,
            };
            c.println(&format!("Deleted {deleted} rows"));
        }
        Some(Verb::List) => {
            let mut lines = match call("LIST", None, &args)?.as_deref() {
                Some([(_, Value::StringList(lines))]) => lines.clone(),
                _ => Vec::new(),
            };
            // `Collections.sort`: by UTF-16 code units.
            lines.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
            for line in lines {
                c.println(&line);
            }
        }
        Some(Verb::Reset) if table == "system" => c.eprintln("Invalid table; no reset performed"),
        Some(Verb::Reset) => {
            args.insert("_reset_mode".into(), Value::Int(command.reset_mode));
            if let Some(tag) = &command.tag {
                args.insert("_tag".into(), Value::String(Some(tag.clone())));
            }
            call("RESET", None, &args)?;
        }
        None => {
            c.eprintln("Unspecified command");
            return Ok(-1);
        }
    }
    Ok(0)
}

/// `dumpHelp(pw, false)`.
fn help(c: &mut ShellCommand) {
    for line in [
        "Settings provider (settings) commands:",
        "  help",
        "      Print this help text.",
        "  get [--user <USER_ID> | current] NAMESPACE KEY",
        "      Retrieve the current value of KEY.",
        "  put [--user <USER_ID> | current] NAMESPACE KEY VALUE [TAG] [default] [overrideableByRestore]",
        "      Change the contents of KEY to VALUE.",
        "      TAG to associate with the setting (cannot be default or overrideableByRestore).",
        "      {default} to set as the default, case-insensitive only for global/secure namespace",
        "      {overrideableByRestore} to let the value be overridden by BackupManager on restore operations",
        "  delete [--user <USER_ID> | current] NAMESPACE KEY",
        "      Delete the entry for KEY.",
        "  reset [--user <USER_ID> | current] NAMESPACE {PACKAGE_NAME | RESET_MODE}",
        "      Reset the global/secure table for a package with mode.",
        "      RESET_MODE is one of {untrusted_defaults, untrusted_clear, trusted_defaults}, case-insensitive",
        "  list [--user <USER_ID> | current] NAMESPACE",
        "      Print all defined keys.",
        "      NAMESPACE is one of {system, secure, global}, case-insensitive",
    ] {
        c.println(line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command() -> Command {
        Command {
            user: USER_NULL,
            verb: Some(Verb::Put),
            table: Some("global".into()),
            key: Some("k".into()),
            value: Some("v".into()),
            tag: None,
            reset_mode: -1,
            make_default: false,
            overrideable_by_restore: false,
        }
    }

    /// `put NAMESPACE KEY VALUE [TAG] [default] [overrideableByRestore]`,
    /// each at most once and in that order.
    #[test]
    fn optional_put_arguments_come_in_order() {
        let mut c = command();
        assert!(optional_put_argument(&mut c, "tag") && optional_put_argument(&mut c, "default"));
        assert!(optional_put_argument(&mut c, "overrideableByRestore"));
        assert_eq!((c.tag.as_deref(), c.make_default, c.overrideable_by_restore), (Some("tag"), true, true));
        let mut c = command();
        assert!(optional_put_argument(&mut c, "default") && !optional_put_argument(&mut c, "tag"));
        let mut c = command();
        assert!(optional_put_argument(&mut c, "overrideableByRestore") && !optional_put_argument(&mut c, "default"));
        let mut c = command();
        assert!(optional_put_argument(&mut c, "a") && !optional_put_argument(&mut c, "b"));
    }

    #[test]
    fn user_arguments() {
        assert_eq!(parse_user_arg("cur"), Ok(USER_CURRENT));
        assert_eq!(parse_user_arg("all"), Ok(USER_ALL));
        assert_eq!(parse_user_arg("10"), Ok(10));
        assert_eq!(parse_user_arg("x"), Err("java.lang.IllegalArgumentException: Bad user number: x".into()));
    }
}
