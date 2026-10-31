//! DeviceConfigService.MyShellCommand, android-16.0.0_r1 (AOSP Apache-2.0).
//! https://android.googlesource.com/platform/frameworks/base/+/android-16.0.0_r1/packages/SettingsProvider/src/com/android/providers/settings/DeviceConfigService.java
//! Config values, overrides and sync mode all belong to SettingsProvider.
use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Exception, Parcel, UNKNOWN_TRANSACTION};

use super::settings_provider::{Caller, SettingsProvider};
use super::settings_service::java_exception;
use crate::bundle::Value;
use crate::shell::{SHELL_COMMAND_TRANSACTION, ShellCommand};

const OVERRIDES: &str = "device_config_overrides/";
const PRIVATE: [&str; 3] = ["device_config_overrides", "staged", "token_staged"];

pub(super) struct DeviceConfigService(pub Arc<SettingsProvider>);
impl Service for DeviceConfigService {
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
        if call.code != SHELL_COMMAND_TRANSACTION {
            return Err(UNKNOWN_TRANSACTION);
        }
        let mut c =
            ShellCommand::read(&self.0.host.process, &mut call.data).map_err(|_| BAD_VALUE)?;
        if c.has_output() {
            // This override of Binder.onShellCommand has no blanket UID gate.
            let caller = Caller {
                uid: call.sender_euid as i32,
                package: None,
            };
            let result = match run(&self.0, &caller, &mut c) {
                Ok(status) => status,
                Err(e) => {
                    c.exception(&java_exception(&e));
                    -1
                }
            };
            c.finish(result);
        }
        let mut p = Parcel::new();
        p.write_no_exception();
        Ok(p)
    }
}

fn run(p: &SettingsProvider, caller: &Caller, c: &mut ShellCommand) -> Result<i32, Exception> {
    let cmd = c.command().unwrap_or("help").to_string();
    if matches!(cmd.as_str(), "help" | "-h") {
        c.print(HELP);
        return Ok(-1);
    }
    let verb = cmd.to_ascii_lowercase();
    if matches!(verb.as_str(), "list_namespaces" | "list_local_overrides") {
        if verb == "list_namespaces" && c.args[1..].iter().any(|a| a == "--public") {
            for namespace in super::device_config::PUBLIC_NAMESPACES {
                c.println(namespace);
            }
            return Ok(0);
        }
        c.eprintln(&format!(
            "Error: unsupported device_config command '{cmd}' (#2531)"
        ));
        return Ok(-1);
    }
    let args = &c.args[1..];
    let mut mode = -1;
    let mut error = "Bad arguments".to_string();
    let valid = match verb.as_str() {
        "get" | "delete" | "clear_override" => {
            if args.len() > 2 {
                error = "Too many arguments".into();
            }
            args.len() == 2
        }
        "list" => {
            if args.len() > 1 {
                error = "Too many arguments".into();
            }
            args.len() <= 1
        }
        "put" | "override" => {
            let default = args
                .iter()
                .skip(3)
                .position(|a| a.eq_ignore_ascii_case("default"));
            if default.is_some() && (verb != "put" || default != Some(args.len() - 4)) {
                error = "Too many arguments".into();
            }
            args.len() == 3 || (verb == "put" && args.len() > 3 && default == Some(args.len() - 4))
        }
        "reset" | "set_sync_disabled_for_tests" => {
            let reset = verb == "reset";
            let modes: &[&str] = if reset {
                &["untrusted_defaults", "untrusted_clear", "trusted_defaults"]
            } else {
                &["none", "persistent", "until_reboot"]
            };
            if let Some(arg) = args.first() {
                mode = modes
                    .iter()
                    .position(|m| arg.eq_ignore_ascii_case(m))
                    .map(|i| i as i32 + if reset { 2 } else { 0 })
                    .unwrap_or(-1);
                if mode == -1 {
                    let label = if reset { "reset" } else { "sync disabled" };
                    error = format!("Invalid {label} mode: {arg}");
                } else if reset && args.len() > 2 {
                    error = "Too many arguments".into();
                }
            }
            mode != -1 && (args.len() == 1 || (verb == "reset" && args.len() == 2))
        }
        "get_sync_disabled_for_tests" => args.is_empty(),
        _ => {
            error = format!("Invalid command: {cmd}");
            false
        }
    };
    if !valid {
        c.eprintln(&error);
        return Ok(-1);
    }
    let namespace = args.get(if verb == "reset" { 1 } else { 0 }).cloned();
    let key = args.get(1).cloned().unwrap_or_default();
    let value = args.get(2).cloned();
    let default = args.len() > 3;
    let mut extras = HashMap::new();
    let call = |method: &str, name: Option<&str>, args: &HashMap<String, Value>| {
        p.call(caller, method, name, args, None)
    };
    let prefix = namespace.as_ref().map(|n| format!("{n}/"));
    let name = format!("{}{}", prefix.as_deref().unwrap_or_default(), &key);
    if matches!(
        verb.as_str(),
        "put" | "override" | "clear_override" | "delete"
    ) {
        let settings = &p.host.platform()?.settings;
        if let Some(flag) = settings.aconfig_flag(&key) {
            if caller.uid != 0 {
                c.println("Error: must be root to write aconfig flag");
                return Ok(0);
            }
            if !flag.read_write {
                c.println("Error: cannot write read-only flag");
                return Ok(0);
            }
        }
    }
    let name = if matches!(verb.as_str(), "override" | "clear_override") {
        format!("{OVERRIDES}{}:{key}", namespace.as_deref().unwrap())
    } else {
        name
    };
    match verb.as_str() {
        "get" | "list" => {
            let all = flags(p, caller, None)?;
            let mut flags = flags(p, caller, prefix.as_deref())?;
            for (key, value) in all {
                if let Some(key) = key.strip_prefix(OVERRIDES)
                    && value.is_some()
                {
                    let key = key.replacen(':', "/", 1);
                    if prefix.as_deref().is_some_and(|p| key.starts_with(p))
                        || flags.contains_key(&key)
                    {
                        flags.insert(key, value);
                    }
                }
            }
            if verb == "get" {
                let value = flags.get(&name).and_then(|v| v.as_deref());
                c.println(value.unwrap_or("null"));
            } else {
                for (key, value) in flags {
                    if prefix.is_none()
                        && verb == "list"
                        && PRIVATE.iter().any(|p| key.starts_with(p))
                    {
                        continue;
                    }
                    let key = prefix
                        .as_deref()
                        .and_then(|p| key.strip_prefix(p))
                        .unwrap_or(&key);
                    let value = value.as_deref().unwrap_or("null");
                    c.println(&format!("{key}={value}"));
                }
            }
        }
        "put" | "override" => {
            extras.insert("value".into(), Value::String(value));
            extras.insert("_make_default".into(), Value::Bool(default));
            call("PUT_config", Some(&name), &extras)?;
        }
        "delete" | "clear_override" => {
            let result = call("DELETE_config", Some(&name), &extras)?.unwrap_or_default();
            if verb == "delete" {
                let deleted = result
                    .iter()
                    .any(|(k, v)| k == "result_rows_deleted" && matches!(v, Value::Int(1)));
                let action = if deleted {
                    "Successfully deleted"
                } else {
                    "Failed to delete"
                };
                c.println(&format!("{action} {key} from {}", namespace.unwrap()));
            }
        }
        "reset" => {
            extras.insert("_reset_mode".into(), Value::Int(mode));
            extras.insert("_prefix".into(), Value::String(prefix));
            call("RESET_config", None, &extras)?;
        }
        "set_sync_disabled_for_tests" => {
            extras.insert("_disabled_mode".into(), Value::Int(mode));
            call("SET_SYNC_DISABLED_MODE_config", None, &extras)?;
        }
        "get_sync_disabled_for_tests" => {
            let result = call("GET_SYNC_DISABLED_MODE_config", None, &extras)?.unwrap_or_default();
            let mode = match result.as_slice() {
                [(_, Value::Int(mode))] => *mode,
                _ => return Err(Exception::illegal_argument("Missing sync disabled mode")),
            };
            if let Some(mode) = ["none", "persistent", "until_reboot"].get(mode as usize) {
                c.println(mode);
            } else {
                c.eprintln(&format!("Unknown mode: {mode}"));
                return Ok(-1);
            }
        }
        _ => unreachable!(),
    }
    Ok(0)
}

fn flags(
    p: &SettingsProvider,
    caller: &Caller,
    prefix: Option<&str>,
) -> Result<BTreeMap<String, Option<String>>, Exception> {
    let args = HashMap::from([("_prefix".into(), Value::String(prefix.map(str::to_string)))]);
    Ok(p.call(caller, "LIST_config", None, &args, None)?
        .unwrap_or_default()
        .into_iter()
        .find_map(|(k, v)| match v {
            Value::StringMap(flags) if k == "value" => Some(flags.into_iter().collect()),
            _ => None,
        })
        .unwrap_or_default())
}

const HELP: &str = "Device Config (device_config) commands:
  help
      Print this help text.
  get NAMESPACE KEY
      Retrieve the current value of KEY from the given NAMESPACE.
  put NAMESPACE KEY VALUE [default]
      Change the contents of KEY to VALUE for the given NAMESPACE.
      {default} to set as the default value.
  override NAMESPACE KEY VALUE
      Set flag NAMESPACE/KEY to the given VALUE, and ignores server-updates for
      this flag. This can still be called even if there is no underlying value set.
  delete NAMESPACE KEY
      Delete the entry for KEY for the given NAMESPACE.
  clear_override NAMESPACE KEY
      Clear local sticky flag override for KEY in the given NAMESPACE.
  list_namespaces [--public]
      Prints the name of all (or just the public) namespaces.
  list [NAMESPACE]
      Print all keys and values defined, optionally for the given NAMESPACE.
  list_local_overrides
      Print all flags that have been overridden.
  reset RESET_MODE [NAMESPACE]
      Reset all flag values, optionally for a NAMESPACE, according to RESET_MODE.
      RESET_MODE is one of {untrusted_defaults, untrusted_clear, trusted_defaults}
      NAMESPACE limits which flags are reset if provided, otherwise all flags are reset
  set_sync_disabled_for_tests SYNC_DISABLED_MODE
      Modifies bulk property setting behavior for tests. When in one of the disabled modes
      this ensures that config isn't overwritten. SYNC_DISABLED_MODE is one of:
        none: Sync is not disabled. A reboot may be required to restart syncing.
        persistent: Sync is disabled, this state will survive a reboot.
        until_reboot: Sync is disabled until the next reboot.
  get_sync_disabled_for_tests
      Prints one of the SYNC_DISABLED_MODE values, see set_sync_disabled_for_tests
";
