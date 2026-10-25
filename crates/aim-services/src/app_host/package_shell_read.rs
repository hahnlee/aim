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

fn integer(value: &str) -> Result<i32, Exception> {
    crate::package::intent_uri::decode_integer(value)
        .ok_or_else(|| Exception::illegal_argument(format!("Invalid integer: {value}")))
}
fn component(value: &str) -> Result<ComponentName, Exception> {
    let (package, class) = value
        .split_once('/')
        .filter(|(_, c)| !c.is_empty())
        .ok_or_else(|| Exception::illegal_argument(format!("Bad component name: {value}")))?;
    Ok(ComponentName {
        package: package.into(),
        class: if class.starts_with('.') {
            format!("{package}{class}")
        } else {
            class.into()
        },
    })
}

/// The filter fields and scalar extras of Intent's command syntax. Unsupported
/// compound syntax fails explicitly until its codec owner is implemented (#2022).
fn parse(arguments: &[Option<String>]) -> Result<Vec<u8>, Exception> {
    let mut args = arguments
        .iter()
        .map(|s| s.as_deref().unwrap_or_default())
        .peekable();
    let mut intent = FullIntent::default();
    let (mut user, mut flags, mut brief, mut components) = (-2, 0, false, false);
    let mut supplied = false;
    while args.peek().is_some_and(|arg| arg.starts_with('-')) {
        let option = args.next().unwrap();
        if option == "--" {
            break;
        }
        let mut required = || {
            args.next().ok_or_else(|| {
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
                intent.intent.component = Some(component(required()?)?);
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
            "--grant-read-uri-permission" => intent.intent.flags |= 1,
            "--grant-write-uri-permission" => intent.intent.flags |= 2,
            "--grant-persistable-uri-permission" => intent.intent.flags |= 0x40,
            "--grant-prefix-uri-permission" => intent.intent.flags |= 0x80,
            "--debug-log-resolution" => intent.intent.flags |= 8,
            "--exclude-stopped-packages" => intent.intent.flags |= 0x10,
            "--include-stopped-packages" => intent.intent.flags |= 0x20,
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
            _ => {
                return Err(Exception::new(
                    aim_binder_host::parcel::EX_UNSUPPORTED_OPERATION,
                    format!("Intent command option {option} (#2022)"),
                ));
            }
        }
    }
    if args.next().is_some() {
        return Err(Exception::new(
            aim_binder_host::parcel::EX_UNSUPPORTED_OPERATION,
            "Intent command positional URI/package/component codec (#2022)",
        ));
    }
    if !supplied {
        return Err(Exception::illegal_argument("No intent supplied"));
    }
    let mut out = Parcel::new();
    out.write_i32(user);
    out.write_i32(flags);
    out.write_bool(brief);
    out.write_bool(components);
    intent.write(&mut out);
    Ok(out.data().to_vec())
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
        for args in [vec![], vec!["--brief"], vec!["-a"], vec!["-n", "invalid"]] {
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
}
