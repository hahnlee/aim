//! Independent shell resources and original Java Intent command syntax.
//! Intent.parseCommandArgs and PackageManagerShellCommand.makeIntent,
//! android-16.0.0_r1, AOSP Apache-2.0. Package resolution stays in the session.
use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Exception, Parcel};
use aim_service_aidl::dev_aim_server_ipackageshellreadleaf as leaf;

use super::{AppHost, reply};
use aim_service_aidl::dev_aim_server_ipackageintentparser as parser;

pub struct ShellRead(pub Arc<AppHost>);

impl ShellRead {
    fn parse(&self, arguments: Option<Vec<Option<String>>>) -> Result<Option<Vec<u8>>, Exception> {
        let owner = self
            .0
            .package_intent_parser
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "package Intent parser unavailable",
                )
            })?;
        let mut request = Parcel::new();
        parser::ParseIntent { arguments }.write(&mut request);
        let transport = |status| {
            Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                format!("package Intent parser: binder status {status}"),
            )
        };
        let result = owner
            .transact_as_owner(parser::PARSE_INTENT, &request, false)
            .map_err(transport)?;
        parser::read_parse_intent_reply(&mut result.reader()).map_err(transport)?
    }

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
                self.parse(a.arguments)
                    .map(|v| leaf::write_parse_intent_reply(&mut p, &v))
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
    use aim_binder_host::parcel::{
        EX_ILLEGAL_ARGUMENT, EX_ILLEGAL_STATE, EX_NULL_POINTER, EX_SECURITY,
    };
    use aim_service_aidl::dev_aim_server_isystemprocess as sp;
    use std::sync::Mutex;

    struct Parser(Mutex<Vec<Option<Vec<Option<String>>>>>);
    impl Service for Parser {
        fn descriptor(&self) -> &str {
            parser::DESCRIPTOR
        }
        fn transact(&self, call: &mut Call<'_>) -> Reply {
            assert_eq!(call.sender_euid, 1000);
            assert_eq!(call.code, parser::PARSE_INTENT);
            let args = parser::ParseIntent::read(&mut call.data)?.arguments;
            self.0.lock().unwrap().push(args.clone());
            if args.as_ref().is_some_and(|v| v.is_empty()) {
                return reply(Err(Exception::illegal_argument("No intent supplied")));
            }
            let mut p = Parcel::new();
            // Opaque original record; forwarding must not decode/reencode it.
            parser::write_parse_intent_reply(&mut p, &Some(vec![0, 255, 1, 128]));
            Ok(p)
        }
    }

    fn call(service: &dyn Service, uid: u32, code: u32, request: &Parcel) -> Parcel {
        service
            .transact(&mut Call {
                code,
                flags: 0,
                sender_pid: 42,
                sender_euid: uid,
                data: request.reader(),
            })
            .unwrap()
    }

    fn options_reach_parser(options: &[&str]) {
        crate::system::with_discovery_binders(|host, _, _, export| {
            let parser = Arc::new(Parser(Mutex::new(Vec::new())));
            let owner = export(parser.clone());
            let endpoint = super::super::system_process::SystemProcess::new(host.clone());
            let shell = ShellRead(host.clone());
            let mut attach = Parcel::new();
            sp::AttachPackageIntentParser {
                parser: Some(owner.binder()),
            }
            .write(&mut attach);
            let denied = call(&endpoint, 2000, sp::ATTACH_PACKAGE_INTENT_PARSER, &attach);
            assert_eq!(
                denied.reader().read_exception().unwrap().unwrap_err().code,
                EX_SECURITY
            );
            assert_eq!(
                shell.parse(Some(vec![])).unwrap_err().code,
                EX_ILLEGAL_STATE
            );
            let mut null = Parcel::new();
            sp::AttachPackageIntentParser { parser: None }.write(&mut null);
            let null = call(&endpoint, 1000, sp::ATTACH_PACKAGE_INTENT_PARSER, &null);
            assert_eq!(
                null.reader().read_exception().unwrap().unwrap_err().code,
                EX_NULL_POINTER
            );
            call(&endpoint, 1000, sp::ATTACH_PACKAGE_INTENT_PARSER, &attach)
                .reader()
                .read_exception()
                .unwrap()
                .unwrap();

            let arguments = Some(options.iter().map(|v| Some((*v).into())).collect());
            let mut request = Parcel::new();
            leaf::ParseIntent {
                arguments: arguments.clone(),
            }
            .write(&mut request);
            let denied = call(&shell, 2000, leaf::PARSE_INTENT, &request);
            assert_eq!(
                denied.reader().read_exception().unwrap().unwrap_err().code,
                EX_SECURITY
            );
            let before = parser.0.lock().unwrap().len();
            let answer = call(&shell, 1000, leaf::PARSE_INTENT, &request);
            assert_eq!(
                leaf::read_parse_intent_reply(&mut answer.reader())
                    .unwrap()
                    .unwrap(),
                Some(vec![0, 255, 1, 128])
            );
            let calls = parser.0.lock().unwrap();
            assert_eq!(calls.len(), before + 1);
            assert_eq!(calls.last(), Some(&arguments));
            drop(calls);
            let error = shell.parse(Some(vec![])).unwrap_err();
            assert_eq!(
                (error.code, error.message.as_str()),
                (EX_ILLEGAL_ARGUMENT, "No intent supplied")
            );
        });
    }

    macro_rules! option_family {
        ($name:ident, $($arg:literal),+ $(,)?) => {
            #[test]
            fn $name() { options_reach_parser(&[$($arg),+]); }
        };
    }
    option_family!(selector, "--selector", "-a", "android.intent.action.VIEW");
    option_family!(uri_extra, "--eu", "uri", "content://a/path", "-a", "VIEW");
    option_family!(component_extra, "--ecn", "component", "a/.B", "-a", "VIEW");
    option_family!(double_extra, "--ed", "double", "-0.125");
    option_family!(
        integer_arrays_and_lists,
        "--eia",
        "ints",
        "0x10,-2",
        "--eial",
        "intlist",
        "3,4",
        "-a",
        "VIEW"
    );
    option_family!(
        long_arrays_and_lists,
        "--ela",
        "longs",
        "1,-2",
        "--elal",
        "longlist",
        "3,4"
    );
    option_family!(
        float_arrays_and_lists,
        "--efa",
        "floats",
        "1.5,NaN",
        "--efal",
        "floatlist",
        "3,4"
    );
    option_family!(
        double_arrays_and_lists,
        "--eda",
        "doubles",
        "1.5,Infinity",
        "--edal",
        "doublelist",
        "3,4"
    );
    option_family!(
        string_arrays_and_lists,
        "--esa",
        "strings",
        "a\\,b,c",
        "--esal",
        "stringlist",
        "d\\,e,f"
    );
    option_family!(
        shell_options,
        "--brief",
        "--components",
        "--user",
        "current",
        "--query-flags",
        "0x10000",
        "a.b"
    );
}
