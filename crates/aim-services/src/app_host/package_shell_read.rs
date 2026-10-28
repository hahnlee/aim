//! Independent shell syntax and resources for the native package front.
//! Port of Intent.parseCommandArgs and PackageManagerShellCommand.makeIntent,
//! android-16.0.0_r1, AOSP Apache-2.0. Package resolution stays in the session.
use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Exception, Parcel};
use aim_service_aidl::dev_aim_server_ipackageshellreadleaf as leaf;

use super::{AppHost, reply};
use crate::bundle::Value;
use crate::package::{intent::ComponentName, intent_uri::FullIntent, uri::Uri};

pub struct ShellRead(pub Arc<AppHost>);

fn codec(status: i32) -> Exception {
    Exception::new(
        aim_binder_host::parcel::EX_ILLEGAL_STATE,
        format!("shell syntax codec: {status}"),
    )
}
fn integer(value: &str) -> Result<i32, Exception> {
    crate::package::intent_uri::decode_integer(value)
        .ok_or_else(|| Exception::illegal_argument(format!("Invalid integer: {value}")))
}
fn component(value: &str) -> Option<ComponentName> {
    let (package, class) = value.split_once('/').filter(|(_, c)| !c.is_empty())?;
    Some(ComponentName {
        package: package.into(),
        class: if class.starts_with('.') {
            format!("{package}{class}")
        } else {
            class.into()
        },
    })
}

/// The filter fields and scalar extras of Intent's command syntax. Unsupported
/// compound syntax fails explicitly until its codec owner is implemented (#2331).
fn parse(arguments: &[Option<String>]) -> Result<Vec<u8>, Exception> {
    let mut args = arguments
        .iter()
        .map(|s| s.as_deref().unwrap_or_default())
        .collect::<std::collections::VecDeque<_>>();
    let mut intent = FullIntent::default();
    let (mut user, mut flags, mut brief, mut components) = (-2, 0, false, false);
    let mut supplied = false;
    while args.front().is_some_and(|arg| arg.starts_with('-')) {
        let raw = args.pop_front().unwrap();
        let option = if !raw.starts_with("--") && raw.len() > 2 && raw.is_char_boundary(2) {
            args.push_front(&raw[2..]);
            &raw[..2]
        } else {
            raw
        };
        if option == "--" {
            break;
        }
        let mut required = || {
            args.pop_front().ok_or_else(|| {
                Exception::illegal_argument(format!("Argument expected after {option}"))
            })
        };
        match option {
            "--user" => {
                user = match required()? {
                    "all" => -1,
                    "current" | "cur" => -2,
                    v => v.parse().map_err(|_| {
                        Exception::illegal_argument(format!("Bad user number: {v}"))
                    })?,
                }
            }
            "--query-flags" => flags = integer(required()?)?,
            "--brief" => brief = true,
            "--components" => components = true,
            "-a" => {
                intent.intent.action = Some(required()?.into());
                supplied = true;
            }
            "-d" => {
                intent.intent.data = Some(Uri::parse(required()?));
                supplied = true;
            }
            "-t" => {
                intent.intent.ty = Some(required()?.into());
                supplied = true;
            }
            "-i" => {
                intent.intent.identifier = Some(required()?.into());
                supplied = true;
            }
            "-p" => {
                intent.intent.package = Some(required()?.into());
                supplied = true;
            }
            "-n" => {
                let value = required()?;
                intent.intent.component = Some(component(value).ok_or_else(|| {
                    Exception::illegal_argument(format!("Bad component name: {value}"))
                })?);
                supplied = true;
            }
            "-c" => {
                let category = required()?.to_owned();
                let categories = intent.intent.categories.get_or_insert_with(Vec::new);
                if !categories.contains(&category) {
                    categories.push(category);
                }
                supplied = true;
            }
            "-f" => intent.intent.flags = integer(required()?)?,
            "--grant-read-uri-permission" => intent.intent.flags |= 0x00000001u32 as i32,
            "--grant-write-uri-permission" => intent.intent.flags |= 0x00000002u32 as i32,
            "--grant-persistable-uri-permission" => intent.intent.flags |= 0x00000040u32 as i32,
            "--grant-prefix-uri-permission" => intent.intent.flags |= 0x00000080u32 as i32,
            "--exclude-stopped-packages" => intent.intent.flags |= 0x00000010u32 as i32,
            "--include-stopped-packages" => intent.intent.flags |= 0x00000020u32 as i32,
            "--debug-log-resolution" => intent.intent.flags |= 0x00000008u32 as i32,
            "--activity-brought-to-front" => intent.intent.flags |= 0x00400000u32 as i32,
            "--activity-clear-top" => intent.intent.flags |= 0x04000000u32 as i32,
            "--activity-clear-when-task-reset" => intent.intent.flags |= 0x00080000u32 as i32,
            "--activity-exclude-from-recents" => intent.intent.flags |= 0x00800000u32 as i32,
            "--activity-launched-from-history" => intent.intent.flags |= 0x00100000u32 as i32,
            "--activity-multiple-task" => intent.intent.flags |= 0x08000000u32 as i32,
            "--activity-no-animation" => intent.intent.flags |= 0x00010000,
            "--activity-reorder-to-front" => intent.intent.flags |= 0x00020000,
            "--activity-clear-task" => intent.intent.flags |= 0x00008000,
            "--activity-task-on-home" => intent.intent.flags |= 0x00004000,
            "--activity-no-history" => intent.intent.flags |= 0x40000000u32 as i32,
            "--activity-no-user-action" => intent.intent.flags |= 0x00040000u32 as i32,
            "--activity-previous-is-top" => intent.intent.flags |= 0x01000000u32 as i32,
            "--activity-reset-task-if-needed" => intent.intent.flags |= 0x00200000u32 as i32,
            "--activity-single-top" => intent.intent.flags |= 0x20000000u32 as i32,
            "--activity-match-external" => intent.intent.flags |= 0x00000800u32 as i32,
            "--receiver-registered-only" => intent.intent.flags |= 0x40000000u32 as i32,
            "--receiver-replace-pending" => intent.intent.flags |= 0x20000000u32 as i32,
            "--receiver-foreground" => intent.intent.flags |= 0x10000000u32 as i32,
            "--receiver-no-abort" => intent.intent.flags |= 0x08000000u32 as i32,
            "--receiver-include-background" => intent.intent.flags |= 0x01000000u32 as i32,
            "-e" | "--es" | "--esn" | "--ei" | "--el" | "--ef" | "--ez" => {
                let key = required()?.to_owned();
                let value = match option {
                    "--esn" => Value::String(None),
                    "--ei" => Value::Int(integer(required()?)?),
                    "--el" => Value::Long(
                        required()?
                            .parse()
                            .map_err(|_| Exception::illegal_argument("Invalid long"))?,
                    ),
                    "--ef" => {
                        supplied = true;
                        Value::Float(
                            required()?
                                .parse()
                                .map_err(|_| Exception::illegal_argument("Invalid float"))?,
                        )
                    }
                    "--ez" => {
                        let value = required()?.to_lowercase();
                        Value::Bool(match value.as_str() {
                            "true" | "t" => true,
                            "false" | "f" => false,
                            _ => {
                                integer(&value).map_err(|_| {
                                    Exception::illegal_argument(format!(
                                        "Invalid boolean value: {value}"
                                    ))
                                })? != 0
                            }
                        })
                    }
                    _ => Value::String(Some(required()?.into())),
                };
                let extras = intent.extras.get_or_insert_with(Vec::new);
                if let Some(entry) = extras.iter_mut().find(|(name, _)| name == &key) {
                    entry.1 = value;
                } else {
                    extras.push((key, value));
                }
            }
            "--selector" | "--eu" | "--ecn" | "--eia" | "--eial" | "--ela" | "--elal" | "--efa"
            | "--efal" | "--ed" | "--eda" | "--edal" | "--esa" | "--esal" => {
                return Err(Exception::new(
                    aim_binder_host::parcel::EX_UNSUPPORTED_OPERATION,
                    format!("Intent command option {option} (#2331)"),
                ));
            }
            _ => {
                return Err(Exception::illegal_argument(format!(
                    "Unknown option: {option}"
                )));
            }
        }
    }
    let positional = args.pop_front();
    let mut base = if let Some(value) = positional {
        supplied = true;
        if value.contains(':') {
            FullIntent::parse_uri(value, 7).map_err(Exception::illegal_argument)?
        } else {
            let mut base = FullIntent::default();
            base.intent.action = Some("android.intent.action.MAIN".into());
            base.intent.categories = Some(vec!["android.intent.category.LAUNCHER".into()]);
            if value.contains('/') {
                base.intent.component = component(value);
            } else {
                base.intent.package = Some(value.into());
            }
            base
        }
    } else {
        FullIntent::default()
    };
    if intent.intent.action.is_some() {
        base.intent.categories = None;
    }
    if !supplied {
        return Err(Exception::illegal_argument("No intent supplied"));
    }
    let mut out = Parcel::new();
    out.write_i32(user);
    out.write_i32(flags);
    out.write_bool(brief);
    out.write_bool(components);
    if positional.is_some() {
        use super::kept::{FILL_IN_COMPONENT, FILL_IN_SELECTOR, RawIntent};
        use aim_service_aidl::{ReadParcelable, WriteParcelable};
        let mut mine = Parcel::new();
        intent.write(&mut mine);
        let mut theirs = Parcel::new();
        base.write(&mut theirs);
        let mine = RawIntent::read_from(&mut mine.reader()).map_err(codec)?;
        let theirs = RawIntent::read_from(&mut theirs.reader()).map_err(codec)?;
        mine.fill_in(&theirs, FILL_IN_COMPONENT | FILL_IN_SELECTOR)
            .map_err(codec)?
            .0
            .write_to(&mut out);
    } else {
        intent.write(&mut out);
    }
    Ok(out.data().to_vec())
}

impl ShellRead {
    fn instrumentation(
        &self,
        a: leaf::ListInstrumentation,
    ) -> Result<Vec<Option<String>>, Exception> {
        let capture = self.0.package_capture()?;
        let resolution = super::package_resolution(&capture, "instrumentation")?;
        let query = crate::package::query::Query {
            state: capture.state(),
            filter: &resolution.apps_filter,
            calling_uid: a.calling_uid,
        };
        let mut values = query
            .instrumentations(
                a.target_package.as_deref(),
                crate::package::info::flags::MATCH_KNOWN_PACKAGES,
                0,
            )
            .map_err(|e| self.0.unimplemented_named("package shell read", e.0))??;
        values.sort_by(|a, b| a.0.target_package.cmp(&b.0.target_package));
        values
            .into_iter()
            .map(|v| {
                let info = v.0;
                let missing = || {
                    Exception::new(
                        aim_binder_host::parcel::EX_NULL_POINTER,
                        "instrumentation identity",
                    )
                };
                let package = info.item.package_name.ok_or_else(missing)?;
                let name = info.item.name.ok_or_else(missing)?;
                let name = name
                    .strip_prefix(&package)
                    .filter(|s| s.starts_with('.'))
                    .unwrap_or(&name);
                let target = info.target_package.ok_or_else(missing)?;
                Ok(Some(format!(
                    "instrumentation:{}{package}/{name} (target={target})",
                    if a.show_source_dir {
                        format!("{}=", info.source_dir.as_deref().unwrap_or("null"))
                    } else {
                        String::new()
                    }
                )))
            })
            .collect()
    }
}

impl Service for ShellRead {
    fn descriptor(&self) -> &str {
        leaf::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if call.sender_euid != 1000 {
            return reply(Err(Exception::security("Native shell owner required")));
        }
        let mut p = Parcel::new();
        let result = match call.code {
            leaf::PARSE_INTENT => {
                let a = leaf::ParseIntent::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                a.arguments
                    .ok_or_else(|| {
                        Exception::new(aim_binder_host::parcel::EX_NULL_POINTER, "arguments")
                    })
                    .and_then(|args| parse(&args))
                    .map(|v| leaf::write_parse_intent_reply(&mut p, &Some(v)))
            }
            leaf::LIST_INSTRUMENTATION => {
                let a = leaf::ListInstrumentation::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.instrumentation(a)
                    .map(|v| leaf::write_list_instrumentation_reply(&mut p, &Some(v)))
            }
            leaf::GET_MAX_SUPPORTED_USERS => {
                leaf::GetMaxSupportedUsers::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                super::user_service::max_supported_users(&self.0)
                    .map(|v| leaf::write_get_max_supported_users_reply(&mut p, v))
            }
            code => Err(self
                .0
                .unimplemented("package shell read", leaf::DESCRIPTOR, code)),
        };
        reply(result.map(|()| p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::{intent::Intent, intent_filter::Plain};
    use aim_binder_host::parcel::Reader;

    #[test]
    fn home_command_record_preserves_shell_options_and_intent() {
        let args = [
            "--brief",
            "--user",
            "current",
            "--query-flags",
            "0x10000",
            "-a",
            "android.intent.action.MAIN",
            "-c",
            "android.intent.category.HOME",
            "-c",
            "android.intent.category.HOME",
        ];
        let bytes = parse(&args.map(|s| Some(s.into()))).unwrap();
        let mut r = Reader::new(&bytes, &[]);
        assert_eq!(r.read_i32().unwrap(), -2);
        assert_eq!(r.read_i32().unwrap(), 0x10000);
        assert!(r.read_bool().unwrap());
        assert!(!r.read_bool().unwrap());
        let intent = Intent::read(&mut r, &mut Plain).unwrap();
        assert_eq!(intent.action.as_deref(), Some("android.intent.action.MAIN"));
        assert_eq!(
            intent.categories,
            Some(vec!["android.intent.category.HOME".into()])
        );
        assert_eq!(r.remaining(), 0);
    }

    #[test]
    fn invalid_and_unserved_syntax_never_returns_an_empty_intent() {
        for args in [
            vec![],
            vec!["--brief"],
            vec!["-a"],
            vec!["-n", "invalid"],
            vec!["--unknown"],
        ] {
            let args = args.into_iter().map(|s| Some(s.into())).collect::<Vec<_>>();
            assert_eq!(
                parse(&args).unwrap_err().code,
                aim_binder_host::parcel::EX_ILLEGAL_ARGUMENT
            );
        }
        let args = [Some("--selector".into())];
        assert_eq!(
            parse(&args).unwrap_err().code,
            aim_binder_host::parcel::EX_UNSUPPORTED_OPERATION
        );
    }

    #[test]
    fn positional_package_and_component_use_the_pinned_fill_in_rules() {
        for (args, action, category, package, class) in [
            (
                vec!["a.b"],
                "android.intent.action.MAIN",
                Some("android.intent.category.LAUNCHER"),
                Some("a.b"),
                None,
            ),
            (
                vec![
                    "-aandroid.intent.action.VIEW",
                    "--activity-task-on-home",
                    "-n",
                    "old/.Main",
                    "new/.View",
                ],
                "android.intent.action.VIEW",
                None,
                None,
                Some("new.View"),
            ),
        ] {
            let bytes =
                parse(&args.into_iter().map(|s| Some(s.into())).collect::<Vec<_>>()).unwrap();
            let mut r = Reader::new(&bytes, &[]);
            r.read_i32().unwrap();
            r.read_i32().unwrap();
            r.read_bool().unwrap();
            r.read_bool().unwrap();
            let intent = Intent::read(&mut r, &mut Plain).unwrap();
            assert_eq!(intent.action.as_deref(), Some(action));
            assert_eq!(intent.flags, if class.is_some() { 0x4000 } else { 0 });
            assert_eq!(intent.package.as_deref(), package);
            assert_eq!(
                intent
                    .categories
                    .as_ref()
                    .and_then(|c| c.first().map(String::as_str)),
                category
            );
            assert_eq!(intent.component.as_ref().map(|c| c.class.as_str()), class);
            assert_eq!(r.remaining(), 0);
        }
    }
}
