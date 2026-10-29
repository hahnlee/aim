//! Independent shell resources and original Java Intent command syntax.
//! Intent.parseCommandArgs and PackageManagerShellCommand.makeIntent,
//! android-16.0.0_r1, AOSP Apache-2.0. Package resolution stays in the session.
use std::sync::{Arc, Mutex};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Binder, EX_ILLEGAL_STATE, Exception, Parcel, Reader};
use aim_service_aidl::dev_aim_server_ipackageshellreadleaf as leaf;
use aim_service_aidl::{ReadParcelable, WriteParcelable};
use aim_service_aidl::{
    android_content_pm_ipackagemanager as pm, android_permission_ipermissionmanager as permission,
    dev_aim_server_iapphostpackageshellread as cold,
};

use super::{AppHost, reply};
use aim_service_aidl::dev_aim_server_ipackageintentparser as parser;

pub struct ShellRead(pub Arc<AppHost>);

struct Descriptor(u32);
impl ReadParcelable for Descriptor {
    fn read_from(reader: &mut Reader<'_>) -> aim_binder_host::parcel::Result<Self> {
        if reader.read_i32()? != 0 {
            return Err(BAD_VALUE);
        }
        reader.read_fd().map(Self)
    }
}
struct OutputFile(aim_binder_driver::File);
impl WriteParcelable for OutputFile {
    fn write_to(&self, parcel: &mut Parcel) {
        parcel.write_i32(0);
        parcel.write_file(self.0.clone());
    }
}

fn codec(status: i32) -> Exception {
    Exception::new(
        aim_binder_host::parcel::EX_ILLEGAL_STATE,
        format!("shell syntax codec: {status}"),
    )
}
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

impl AppHost {
    pub(super) fn attach_package_shell_read(
        self: &Arc<Self>,
        binder: Option<Binder>,
    ) -> Result<(), Exception> {
        let Some(Binder::Handle(handle)) = binder else {
            return Err(Exception::illegal_argument("no package shell read owner"));
        };
        let owner = Arc::new(self.process.strong(handle));
        let weak = Arc::downgrade(self);
        self.process.link_to_death(
            &owner,
            Box::new(move || {
                if let Some(host) = weak.upgrade() {
                    let mut current = host.package_shell_read.lock().unwrap();
                    if current.as_ref().is_some_and(|p| p.handle == handle) {
                        current.take();
                    }
                }
            }),
        );
        *self.package_shell_read.lock().unwrap() = Some(owner);
        Ok(())
    }
}

// Only these read operations may replay the original caller inside aim-system.
// Retire the capabilities on return, including a failed original/transport call.
struct ScopedQueries {
    owner: Mutex<Option<Arc<dyn Service>>>,
    uid: i32,
    pid: i32,
    allowed: &'static [u32],
    descriptor: &'static str,
}
impl Service for ScopedQueries {
    fn accepts_fds(&self) -> bool {
        self.allowed
            .contains(&crate::package::dump::DUMP_TRANSACTION)
    }
    fn descriptor(&self) -> &str {
        self.descriptor
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if call.sender_euid != 1000 || !self.allowed.contains(&call.code) {
            return reply(Err(Exception::security("shell read capability required")));
        }
        let owner = self.owner.lock().unwrap().clone();
        let Some(owner) = owner else {
            return reply(Err(Exception::new(
                EX_ILLEGAL_STATE,
                "shell read capability retired",
            )));
        };
        owner.transact(&mut Call {
            code: call.code,
            flags: call.flags,
            sender_euid: self.uid as u32,
            sender_pid: self.pid,
            data: call.data.clone(),
        })
    }
}
struct Scoped(Vec<Arc<ScopedQueries>>);
impl Drop for Scoped {
    fn drop(&mut self) {
        for scope in &self.0 {
            scope.owner.lock().unwrap().take();
        }
    }
}
impl ShellRead {
    fn run_command(&self, call: &mut Call<'_>) -> Result<i32, Exception> {
        if !matches!(call.sender_euid, 0 | 2000)
            || self.0.process.authenticated_inbound_identity()
                != Some((call.sender_pid, call.sender_euid))
        {
            return Err(Exception::security(
                "read shell caller lacks authenticated provenance",
            ));
        }
        let args = leaf::RunReadCommand::<Descriptor>::read(&mut call.data).map_err(codec)?;
        if call.data.remaining() != 0 {
            return Err(Exception::illegal_argument("read shell argument tail"));
        }
        let file = |value: Option<Descriptor>| -> Result<OutputFile, Exception> {
            value
                .and_then(|value| self.0.process.file(value.0))
                .map(OutputFile)
                .ok_or_else(|| Exception::illegal_argument("missing read shell descriptor"))
        };
        if args
            .arguments
            .as_ref()
            .and_then(|a| a.first())
            .and_then(|a| a.as_deref())
            .is_some_and(|a| {
                matches!(
                    a,
                    "get-app-links"
                        | "get-app-link-owners"
                        | "set-app-links-user-selection"
                        | "set-app-links-allowed"
                )
            })
        {
            return super::domain_shell::run(
                &self.0,
                call,
                true,
                args.arguments,
                [
                    file(args.input)?.0,
                    file(args.output)?.0,
                    file(args.error)?.0,
                ],
            );
        }
        let (packages, _) =
            crate::package::service::PackageQueries::from_system(self.0.package_system()?);
        let scoped = Arc::new(ScopedQueries {
            owner: Mutex::new(Some(packages)),
            uid: call.sender_euid as i32,
            pid: call.sender_pid,
            allowed: &[
                pm::GET_PACKAGE_INFO,
                pm::GET_APPLICATION_INFO,
                crate::package::dump::DUMP_TRANSACTION,
            ],
            descriptor: pm::DESCRIPTOR,
        });
        let mut retire = Scoped(vec![scoped.clone()]);
        let [activity, meminfo, procstats, usagestats, batterystats] = if args
            .arguments
            .as_ref()
            .and_then(|args| args.first())
            .and_then(|arg| arg.as_deref())
            == Some("dump")
        {
            let services = self.0.services();
            [
                "activity",
                "meminfo",
                "procstats",
                "usagestats",
                "batterystats",
            ]
            .map(|name| {
                services
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, service)| {
                        let scope = Arc::new(ScopedQueries {
                            owner: Mutex::new(Some(service.clone())),
                            uid: call.sender_euid as i32,
                            pid: call.sender_pid,
                            allowed: &[crate::package::dump::DUMP_TRANSACTION],
                            descriptor: "android.os.IBinder",
                        });
                        retire.0.push(scope.clone());
                        self.0.process.add_callback(scope)
                    })
            })
        } else {
            [None; 5]
        };
        let mut request = Parcel::new();
        cold::RunReadCommand {
            arguments: args.arguments,
            packages: Some(self.0.process.add_callback(scoped)),
            activity,
            meminfo,
            procstats,
            usagestats,
            batterystats,
            input: Some(file(args.input)?),
            output: Some(file(args.output)?),
            error: Some(file(args.error)?),
        }
        .write(&mut request);
        let owner = self
            .0
            .package_shell_read
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| {
                Exception::new(EX_ILLEGAL_STATE, "package shell read owner is not attached")
            })?;
        let result = self
            .0
            .process
            .transact_preserving_inbound(owner.handle, cold::RUN_READ_COMMAND, &request)
            .map_err(codec)?;
        let mut reader = result.reader();
        let status = cold::read_run_read_command_reply(&mut reader).map_err(codec)??;
        if reader.remaining() != 0 {
            return Err(Exception::illegal_argument("read shell reply tail"));
        }
        Ok(status)
    }
    fn cold(&self, code: u32, request: Parcel) -> Result<Parcel, Exception> {
        let owner = self
            .0
            .package_shell_read
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| {
                Exception::new(EX_ILLEGAL_STATE, "package shell read owner is not attached")
            })?;
        owner
            .transact(code, &request, false)
            .map(|r| r.into_parcel())
            .map_err(codec)
    }
    fn permissions(
        &self,
        uid: i32,
        pid: i32,
        a: Option<leaf::ListPermissions>,
    ) -> Result<Option<Vec<Option<String>>>, Exception> {
        let (packages, _) =
            crate::package::service::PackageQueries::from_system(self.0.package_system()?);
        let permissions =
            self.0.permission_manager.get().cloned().ok_or_else(|| {
                Exception::new(EX_ILLEGAL_STATE, "permission owner is unavailable")
            })?;
        let scoped = Scoped(vec![
            Arc::new(ScopedQueries {
                owner: Mutex::new(Some(packages)),
                uid,
                pid,
                allowed: &[pm::GET_APPLICATION_INFO],
                descriptor: pm::DESCRIPTOR,
            }),
            Arc::new(ScopedQueries {
                owner: Mutex::new(Some(permissions)),
                uid,
                pid,
                allowed: &[
                    permission::GET_ALL_PERMISSION_GROUPS,
                    permission::GET_PERMISSION_GROUP_INFO,
                    permission::QUERY_PERMISSIONS_BY_GROUP,
                ],
                descriptor: permission::DESCRIPTOR,
            }),
        ]);
        let mut request = Parcel::new();
        let permission_binder = Some(self.0.process.add_callback(scoped.0[1].clone()));
        let Some(a) = a else {
            cold::ListPermissionGroups {
                permissions: permission_binder,
            }
            .write(&mut request);
            let r = self.cold(cold::LIST_PERMISSION_GROUPS, request)?;
            return cold::read_list_permission_groups_reply(&mut r.reader()).map_err(codec)?;
        };
        cold::ListPermissions {
            groups: a.groups,
            labels: a.labels,
            summary: a.summary,
            minimum: a.minimum,
            maximum: a.maximum,
            group: a.group,
            packages: Some(self.0.process.add_callback(scoped.0[0].clone())),
            permissions: permission_binder,
        }
        .write(&mut request);
        let r = self.cold(cold::LIST_PERMISSIONS, request)?;
        cold::read_list_permissions_reply(&mut r.reader()).map_err(codec)?
    }
}

impl Service for ShellRead {
    fn accepts_fds(&self) -> bool {
        true
    }
    fn descriptor(&self) -> &str {
        leaf::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if call.code == leaf::RUN_READ_COMMAND {
            let mut p = Parcel::new();
            return reply(self.run_command(call).map(|status| {
                leaf::write_run_read_command_reply(&mut p, status);
                p
            }));
        }
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
            leaf::DUMP_RESOLVE_INFO => {
                let a = leaf::DumpResolveInfo::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let mut request = Parcel::new();
                cold::DumpResolveInfo {
                    record: a.record,
                    prefix: a.prefix,
                }
                .write(&mut request);
                self.cold(cold::DUMP_RESOLVE_INFO, request)
                    .and_then(|r| {
                        cold::read_dump_resolve_info_reply(&mut r.reader()).map_err(codec)?
                    })
                    .map(|lines| leaf::write_dump_resolve_info_reply(&mut p, &lines))
            }
            leaf::LIST_PERMISSION_GROUPS => {
                let a = leaf::ListPermissionGroups::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.permissions(a.calling_uid, a.calling_pid, None)
                    .map(|lines| leaf::write_list_permission_groups_reply(&mut p, &lines))
            }
            leaf::LIST_PERMISSIONS => {
                let a = leaf::ListPermissions::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.permissions(a.calling_uid, a.calling_pid, Some(a))
                    .map(|lines| leaf::write_list_permissions_reply(&mut p, &lines))
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
