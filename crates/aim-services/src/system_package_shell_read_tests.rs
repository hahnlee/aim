//! PackageManagerShellCommand android-16.0.0_r1: real driver shell transactions,
//! separate stdin/stdout/stderr and IResultReceiver. Java presentation is a test
//! double here; device-services compilation/DEX linkage checks the original APIs.
use super::shell_permission_tests::{ContextManager, ResultReceiver};
use super::*;
use aim_binder_host::local::Strong;
use aim_service_aidl::{
    android_os_iservicemanager as sm, dev_aim_server_iapphostpackageshellread as cold,
    dev_aim_server_isystemprocess as sp,
};
use std::{
    io::{Read, Seek, SeekFrom, Write},
    os::fd::AsFd,
};

fn lookup(process: &Arc<LocalProcess>, name: &str) -> Strong {
    let mut p = Parcel::new();
    sm::CheckService {
        name: Some(name.into()),
    }
    .write(&mut p);
    let r = process.transact(0, sm::CHECK_SERVICE, &p, false).unwrap();
    let Some(Binder::Handle(handle)) = sm::read_check_service_reply(&mut r.reader())
        .unwrap()
        .unwrap()
    else {
        panic!("remote service")
    };
    process.strong(handle)
}
struct Descriptor(u32);
impl aim_service_aidl::ReadParcelable for Descriptor {
    fn read_from(r: &mut Reader<'_>) -> aim_binder_host::parcel::Result<Self> {
        assert_eq!(r.read_i32()?, 0);
        r.read_fd().map(Self)
    }
}
struct Cold(Arc<LocalProcess>, Arc<Mutex<Option<Arc<Strong>>>>);
impl Service for Cold {
    fn accepts_fds(&self) -> bool {
        true
    }
    fn descriptor(&self) -> &str {
        cold::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if call.code == cold::RUN_READ_COMMAND {
            assert_eq!(call.sender_euid, 2000);
            assert_eq!(call.sender_pid, 97330);
            return self.command(call);
        }
        assert_eq!(call.sender_euid, 1000);
        let mut p = Parcel::new();
        match call.code {
            cold::DUMP_RESOLVE_INFO => {
                let a = cold::DumpResolveInfo::read(&mut call.data)?;
                assert!(a.record.unwrap().len() > 100);
                cold::write_dump_resolve_info_reply(
                    &mut p,
                    &Some(vec![Some(format!(
                        "{}original ResolveInfo dump",
                        a.prefix.unwrap()
                    ))]),
                );
            }
            cold::LIST_PERMISSION_GROUPS => {
                let a = cold::ListPermissionGroups::read(&mut call.data)?;
                self.groups(a.permissions.unwrap());
                cold::write_list_permission_groups_reply(
                    &mut p,
                    &Some(vec![Some("permission group:p.GROUP".into())]),
                );
            }
            cold::LIST_PERMISSIONS => {
                let a = cold::ListPermissions::read(&mut call.data)?;
                assert!(!a.labels && !a.summary && !a.groups);
                assert_eq!((a.minimum, a.maximum), (-10000, 10000));
                self.groups(a.permissions.unwrap());
                // A write is refused by the same scoped capability before decoding.
                let Some(Binder::Handle(h)) = a.packages else {
                    panic!("resource capability")
                };
                let r = self
                    .0
                    .transact(
                        h,
                        pm::SET_APPLICATION_ENABLED_SETTING,
                        &Parcel::new(),
                        false,
                    )
                    .unwrap();
                assert_eq!(
                    r.reader().read_exception().unwrap().unwrap_err().code,
                    aim_binder_host::parcel::EX_SECURITY
                );
                cold::write_list_permissions_reply(
                    &mut p,
                    &Some(vec![Some("permission:p.TEST".into())]),
                );
            }
            _ => panic!("unexpected cold operation"),
        }
        Ok(p)
    }
}
impl Cold {
    fn file(&self, descriptor: Descriptor) -> aim_binder_host::server::RetainedFd {
        aim_binder_host::server::file_fd(&self.0.file(descriptor.0).unwrap()).unwrap()
    }
    fn command(&self, call: &mut Call<'_>) -> Reply {
        let a = cold::RunReadCommand::<Descriptor>::read(&mut call.data)?;
        assert_eq!(call.data.remaining(), 0);
        let mut input = self.file(a.input.unwrap());
        let mut out = self.file(a.output.unwrap());
        let mut err = self.file(a.error.unwrap());
        let mut text = String::new();
        input.read_to_string(&mut text).unwrap();
        assert!(text.is_empty());
        let Binder::Handle(handle) = a.packages.unwrap() else {
            panic!("package capability")
        };
        let package = Arc::new(self.0.strong(handle));
        *self.1.lock().unwrap() = Some(package.clone());
        let denied = package
            .transact(pm::SET_APPLICATION_ENABLED_SETTING, &Parcel::new(), false)
            .unwrap();
        assert_eq!(
            denied.reader().read_exception().unwrap().unwrap_err().code,
            aim_binder_host::parcel::EX_SECURITY
        );
        let args = a
            .arguments
            .unwrap()
            .into_iter()
            .map(Option::unwrap)
            .collect::<Vec<_>>();
        let cmd = args[0].as_str();
        let status = if cmd != "get-shared-uid-allowlist" && args.len() == 1 {
            writeln!(
                err,
                "Error: no {} specified{}",
                if cmd == "get-signature-permission-allowlist" {
                    "partition"
                } else {
                    "package"
                },
                if cmd.starts_with("dump") { "" } else { "." }
            )
            .unwrap();
            1
        } else {
            match cmd {
                "dump-package" => self.dump(&package, &mut out, &[&args[1]]),
                "dump" => {
                    for (name, binder, args) in [
                        (
                            "package",
                            Some(Binder::Handle(handle)),
                            vec![args[1].as_str()],
                        ),
                        (
                            "activity",
                            a.activity,
                            vec!["-a", "package", args[1].as_str()],
                        ),
                        (
                            "meminfo",
                            a.meminfo,
                            vec!["--local", "--package", args[1].as_str()],
                        ),
                        ("procstats", a.procstats, vec![args[1].as_str()]),
                        ("usagestats", a.usagestats, vec![args[1].as_str()]),
                        ("batterystats", a.batterystats, vec![args[1].as_str()]),
                    ] {
                        if name != "package" {
                            writeln!(out).unwrap();
                        }
                        writeln!(out, "DUMP OF SERVICE {name}:").unwrap();
                        if let Some(Binder::Handle(h)) = binder {
                            self.dump(&self.0.strong(h), &mut out, &args);
                        } else {
                            writeln!(out, "  (Service not found)").unwrap();
                        }
                    }
                }
                "get-privapp-permissions"
                | "get-privapp-deny-permissions"
                | "get-oem-permissions" => {
                    let mut p = Parcel::new();
                    pm::GetApplicationInfo {
                        package_name: Some(args[1].clone()),
                        flags: 0,
                        user_id: 0,
                    }
                    .write(&mut p);
                    let r = package
                        .transact(pm::GET_APPLICATION_INFO, &p, false)
                        .unwrap();
                    let mut reader = r.reader();
                    reader.read_exception().unwrap().unwrap();
                    assert_eq!(reader.read_i32().unwrap(), 1);
                    assert!(reader.remaining() > 100);
                    writeln!(out, "{{}}").unwrap();
                }
                "get-signature-permission-allowlist" | "get-shared-uid-allowlist" => {}
                _ => panic!("unexpected cold shell verb {cmd}"),
            }
            0
        };
        let mut p = Parcel::new();
        cold::write_run_read_command_reply(&mut p, status);
        Ok(p)
    }
    fn dump(&self, owner: &Strong, out: &mut aim_binder_host::server::RetainedFd, args: &[&str]) {
        let mut p = Parcel::new();
        p.write_file(out.try_clone().unwrap().into_file_owner());
        aim_service_aidl::write_string_list(
            &mut p,
            Some(&args.iter().map(|s| Some((*s).into())).collect::<Vec<_>>()),
        );
        let result = owner.transact(crate::package::dump::DUMP_TRANSACTION, &p, false);
        match result {
            Ok(r) if r.reader().read_exception().unwrap().is_ok() => {}
            _ => writeln!(out, "Failure dumping service:").unwrap(),
        }
    }
    fn groups(&self, binder: Binder) {
        use aim_service_aidl::android_permission_ipermissionmanager as permission;
        let Binder::Handle(h) = binder else {
            panic!("permission capability")
        };
        let mut p = Parcel::new();
        permission::GetAllPermissionGroups { flags: 0 }.write(&mut p);
        let r = self
            .0
            .transact(h, permission::GET_ALL_PERMISSION_GROUPS, &p, false)
            .unwrap();
        let mut r = r.reader();
        r.read_exception().unwrap().unwrap();
        assert_eq!(r.read_i32().unwrap(), 1);
        assert_eq!(r.read_i32().unwrap(), 1); // nonempty ParceledListSlice
    }
}
struct Shell {
    session: Session,
    process: Arc<LocalProcess>,
    package: Strong,
    result: Binder,
    statuses: mpsc::Receiver<i32>,
    retained: Arc<Mutex<Option<Arc<Strong>>>>,
}
impl Shell {
    fn new() -> Self {
        let services = Arc::new(Mutex::new(BTreeMap::new()));
        let names = services.clone();
        let fixture = Fixture::with_context(|p, s| {
            Arc::new(ContextManager {
                host: crate::service_host::ServiceHost::new(p.clone(), s),
                services: names,
            })
        });
        let session = session_with_fixture(fixture);
        let f = &session.fixture;
        let mut request = Parcel::new();
        request.write_binder(Some(f.native.add_service(Arc::new(
            crate::app_host::package_shell_read::ShellRead(session.host.clone()),
        ))));
        session
            .bridge
            .owner
            .transact(SET_TEST_SHELL_READ, &request, false)
            .unwrap();
        let image = f.root.join("image");
        let data = f.root.join("data");
        std::fs::create_dir_all(image.join("system/framework")).unwrap();
        std::fs::write(image.join("system/build.prop"), "ro.build.version.sdk=36\n").unwrap();
        std::fs::write(
            image.join("system/framework/framework-res.apk"),
            include_bytes!("../tests/fixtures/discovery/framework.apk"),
        )
        .unwrap();
        crate::app_host::configure_test_host(
            &session.host,
            &image,
            &data,
            &f.root.join("bpf"),
            Box::new(|name| match name {
                "ro.build.version.sdk" => Some("36".into()),
                "ro.build.version.codename" => Some("REL".into()),
                "persist.sys.locale" => Some("en-US".into()),
                "ro.sf.lcd_density" => Some("160".into()),
                _ => None,
            }),
        );
        for path in [
            "misc/apexdata/com.android.permission/access.abx",
            "misc_de/0/apexdata/com.android.permission/access.abx",
        ] {
            let dest = data.join(path);
            std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
            let xml = aim_android_xml::read(b"<access><permissions/></access>").unwrap();
            std::fs::write(dest, aim_android_xml::abx::write(&xml).unwrap()).unwrap();
        }
        std::fs::write(data.join("system/users/userlist.xml"), "<users/>").unwrap();
        std::fs::write(data.join("system/users/0.xml"), "<user id='0' flags='19475' type='android.os.usertype.full.SYSTEM'><name>Kim</name><restrictions/></user>").unwrap();
        let before = f.system.capture_package_queries().unwrap();
        let mut owner = before.scan().owner().clone();
        let mut pkg = (**before.state().packages["p"].pkg.as_ref().unwrap()).clone();
        pkg.base_apk_path = Some("/data/app/p/base.apk".into());
        pkg.receivers = pkg.activities.clone();
        pkg.instrumentations = vec![crate::package::pkg::Instrumentation {
            component: crate::package::pkg::Component {
                name: "p.Tests".into(),
                package_name: "p".into(),
                ..Default::default()
            },
            target_package: Some("p".into()),
            ..Default::default()
        }];
        pkg.permission_groups = vec![crate::package::pkg::PermissionGroup {
            component: crate::package::pkg::Component {
                name: "p.GROUP".into(),
                package_name: "p".into(),
                ..Default::default()
            },
            ..Default::default()
        }];
        owner.load_test_code(pkg);
        complete_owner(&mut owner);
        let mut context = (**before.context()).clone();
        context.scan_version = before.scan().version() + 1;
        context.system.features = vec![("fixture.feature".into(), 2)];
        context.system.diagnostic_dates = Some(Arc::new(crate::package::dump::Dates::new(
            session.bridge.clone(),
            Arc::downgrade(&f.system),
        )));
        context.system.system_permissions = Some(
            [(
                2000,
                [
                    "android.permission.MANAGE_USERS".into(),
                    "android.permission.INTERACT_ACROSS_USERS_FULL".into(),
                    "android.permission.DUMP".into(),
                ]
                .into(),
            )]
            .into(),
        );
        f.system
            .publish_package_scan_with_queries(
                &session.bridge,
                Some(before.scan()),
                owner,
                before.scan().usage().clone(),
                context,
            )
            .unwrap();
        for (name, service) in session.host.services() {
            if name == "activity" {
                let node = f.original[0].add_service(service.clone());
                let Binder::Local(ptr) = node else {
                    unreachable!()
                };
                let mut p = Parcel::new();
                p.write_i64(ptr as i64);
                let r = session
                    .bridge
                    .owner
                    .transact(EXPORT_TEST_BINDER, &p, false)
                    .unwrap();
                let binder = r.reader().read_binder().unwrap().unwrap();
                f.system
                    .services
                    .lock()
                    .unwrap()
                    .insert(name, Arc::new(r.retain_remote_binder(binder).unwrap()));
            }
            services
                .lock()
                .unwrap()
                .insert(name.into(), f.native.add_service(service));
        }
        let original = &f.original[0];
        let retained = Arc::new(Mutex::new(None));
        let node = original.add_service(Arc::new(Cold(original.clone(), retained.clone())));
        let mut p = Parcel::new();
        sp::AttachPackageShellRead { owner: Some(node) }.write(&mut p);
        lookup(original, "aim.system_process")
            .transact(sp::ATTACH_PACKAGE_SHELL_READ, &p, false)
            .unwrap()
            .reader()
            .read_exception()
            .unwrap()
            .unwrap();
        let process = LocalProcess::open(
            &f.driver,
            Device::Binder,
            Credentials {
                pid: 97330,
                euid: 2000,
                security_context: None,
            },
        );
        process.start();
        let package = lookup(&process, "package");
        let (tx, statuses) = mpsc::channel();
        let result = process.add_service(Arc::new(ResultReceiver(tx)));
        Self {
            session,
            process,
            package,
            result,
            statuses,
            retained,
        }
    }
    fn command(&self, args: &[&str]) -> (i32, String, String) {
        let root = &self.session.fixture.root;
        let mut out = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(root.join("out"))
            .unwrap();
        let mut err = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(root.join("err"))
            .unwrap();
        let input = std::fs::File::open("/dev/null").unwrap();
        let mut p = Parcel::new();
        for fd in [input.as_fd(), out.as_fd(), err.as_fd()] {
            p.write_file(aim_binder_host::server::file_from_fd(fd).unwrap());
        }
        p.write_i32(args.len() as i32);
        for arg in args {
            p.write_string16(Some(arg));
        }
        p.write_binder(None);
        p.write_binder(Some(self.result));
        self.package
            .transact(crate::shell::SHELL_COMMAND_TRANSACTION, &p, false)
            .unwrap()
            .reader()
            .read_exception()
            .unwrap()
            .unwrap();
        let status = self.statuses.recv_timeout(Duration::from_secs(5)).unwrap();
        let mut stdout = String::new();
        let mut stderr = String::new();
        out.seek(SeekFrom::Start(0)).unwrap();
        out.read_to_string(&mut stdout).unwrap();
        err.seek(SeekFrom::Start(0)).unwrap();
        err.read_to_string(&mut stderr).unwrap();
        (status, stdout, stderr)
    }
    fn check(&self, args: &[&str], status: i32, out: &str) {
        assert_eq!(
            self.command(args),
            (status, out.into(), String::new()),
            "{args:?}"
        );
    }
}
impl Drop for Shell {
    fn drop(&mut self) {
        self.session
            .fixture
            .driver
            .release(self.process.proc_handle());
    }
}
#[test]
fn list_packages() {
    let s = Shell::new();
    s.check(&["list", "packages", "p"], 0, "package:p\n");
    s.check(
        &[
            "list", "packages", "-f", "-i", "-u", "-U", "--uid", "10100", "--user", "0", "p",
        ],
        0,
        "package:/data/app/p/base.apk=p  installer=null uid:10100\n",
    );
    s.check(&["list", "packages", "-e", "-3", "p"], 0, "package:p\n");
    s.check(
        &["list", "packages", "--uid", "bad"],
        -1,
        "Error: java.lang.NumberFormatException: For input string: \"bad\"\n",
    );
    for flag in ["-d", "-s"] {
        s.check(&["list", "packages", flag, "p"], 0, "");
    }
    s.check(
        &["list", "packages", "--bad"],
        -1,
        "Error: Unknown option: --bad\n",
    );
}
#[test]
fn list_features() {
    Shell::new().check(
        &["list", "features"],
        0,
        "feature:reqGlEsVersion=0x0\nfeature:fixture.feature=2\n",
    );
}
#[test]
fn list_libraries() {
    Shell::new().check(&["list", "libraries"], 0, "");
}
#[test]
fn list_permissions() {
    Shell::new().check(
        &["list", "permissions"],
        0,
        "All Permissions:\n\npermission:p.TEST\n",
    );
}
#[test]
fn list_permission_groups() {
    Shell::new().check(
        &["list", "permission-groups"],
        0,
        "permission group:p.GROUP\n",
    );
}
#[test]
fn list_instrumentation() {
    Shell::new().check(
        &["list", "instrumentation", "p"],
        0,
        "instrumentation:p/.Tests (target=p)\n",
    );
}
#[test]
fn list_users() {
    Shell::new().check(
        &["list", "users"],
        0,
        "Users:\n\tUserInfo{0:Kim:4c13} running\n",
    );
}
#[test]
fn resolve_activity() {
    let s = Shell::new();
    s.check(
        &["resolve-activity", "--components", "-n", "p/.Main"],
        0,
        "p/.Main\n",
    );
    s.check(
        &["resolve-activity", "-n", "p/.Main"],
        0,
        "original ResolveInfo dump\n",
    );
}
#[test]
fn query_activities() {
    Shell::new().check(
        &["query-activities", "--components", "-n", "p/.Main"],
        0,
        "p/.Main\n",
    );
}
#[test]
fn query_receivers() {
    Shell::new().check(
        &["query-receivers", "--components", "-n", "p/.Main"],
        0,
        "p/.Main\n",
    );
}
#[test]
fn path() {
    let s = Shell::new();
    s.check(&["path", "p"], 0, "package:/data/app/p/base.apk\n");
    s.check(&["path", "absent"], 1, "");
}

#[test]
fn dump_package() {
    let s = Shell::new();
    assert_eq!(
        s.command(&["dump-package"]),
        (1, String::new(), "Error: no package specified\n".into())
    );
    s.check(
        &["dump-package", "missing.package"],
        0,
        "Unable to find package: missing.package\n",
    );
    let (status, out, err) = s.command(&["dump-package", "p"]);
    assert_eq!((status, err), (0, String::new()));
    assert!(out.contains("Package [p]"));
    assert!(out.contains("userId=10100"));
    assert!(out.contains("firstInstallTime=fixture-date-"));
    assert!(out.contains("fixture.permission: granted=true, flags=[ USER_SET]"));
    let retained = s.retained.lock().unwrap().take().unwrap();
    let reply = retained
        .transact(
            crate::package::dump::DUMP_TRANSACTION,
            &Parcel::new(),
            false,
        )
        .unwrap();
    assert_eq!(
        reply
            .reader()
            .read_exception()
            .unwrap()
            .unwrap_err()
            .message,
        "shell read capability retired"
    );
}

#[test]
fn dump() {
    let s = Shell::new();
    assert_eq!(
        s.command(&["dump"]),
        (1, String::new(), "Error: no package specified\n".into())
    );
    let (status, out, err) = s.command(&["dump", "missing.package"]);
    assert_eq!((status, err), (0, String::new()));
    assert!(out.starts_with("DUMP OF SERVICE package:\nUnable to find package: missing.package\n"));
    for service in [
        "activity",
        "meminfo",
        "procstats",
        "usagestats",
        "batterystats",
    ] {
        assert!(out.contains(&format!("\nDUMP OF SERVICE {service}:\n")));
    }
}

#[test]
fn dump_package_checks_the_shell_permission_before_reading_state() {
    let s = Shell::new();
    let f = &s.session.fixture;
    let before = f.system.capture_package_queries().unwrap();
    let mut context = (**before.context()).clone();
    context.scan_version = before.scan().version() + 1;
    context
        .system
        .system_permissions
        .as_mut()
        .unwrap()
        .get_mut(&2000)
        .unwrap()
        .remove("android.permission.DUMP");
    f.system
        .publish_package_scan_with_queries(
            &s.session.bridge,
            Some(before.scan()),
            before.scan().owner().clone(),
            before.scan().usage().clone(),
            context,
        )
        .unwrap();
    s.check(&["dump-package", "missing.package"], 0,
        "Permission Denial: can't dump PackageManager from from pid=97330, uid=2000 due to missing android.permission.DUMP permission\n");
}

#[test]
fn get_privapp_permissions() {
    let s = Shell::new();
    s.check(&["get-privapp-permissions", "p"], 0, "{}\n");
    assert_eq!(
        s.command(&["get-privapp-permissions"]),
        (1, String::new(), "Error: no package specified.\n".into())
    );
}
#[test]
fn get_privapp_deny_permissions() {
    Shell::new().check(&["get-privapp-deny-permissions", "p"], 0, "{}\n");
}
#[test]
fn get_oem_permissions() {
    Shell::new().check(&["get-oem-permissions", "p"], 0, "{}\n");
}
#[test]
fn get_signature_permission_allowlist() {
    Shell::new().check(&["get-signature-permission-allowlist", "system"], 0, "");
}
#[test]
fn get_shared_uid_allowlist() {
    Shell::new().check(&["get-shared-uid-allowlist"], 0, "");
}
#[test]
fn list_staged_sessions() {
    use crate::package::installer::{Sessions, endpoint::Endpoint, native::NativeOwners, storage};
    let s = Shell::new();
    let f = &s.session.fixture;
    let inode = aim_storage::guest_inode::GuestInode {
        uid: Some(1000),
        gid: Some(1000),
        mode: Some(0o600),
    };
    let disk = storage::Store::open(
        f.root.join("data"),
        inode,
        inode,
        Arc::new(|_, _| panic!("read-only staged listing attempted a write")),
    )
    .unwrap();
    let system = Arc::downgrade(&f.system);
    let process = f.native.clone();
    let owner = NativeOwners::open(
        Arc::new(Sessions::default()),
        Arc::new(move || {
            system
                .upgrade()
                .unwrap()
                .capture_package_queries()
                .map(|c| c.state().clone())
        }),
        Arc::new(|_, _, _| panic!("read-only listing attempted install policy")),
        Default::default(),
        disk,
        Arc::new(move |service| Ok(process.add_service(service))),
        Arc::new(|_| panic!("read-only listing emitted a session event")),
        f.native.clone(),
    )
    .unwrap();
    let binder = f.native.add_service(Arc::new(Endpoint {
        sessions: owner.sessions.clone(),
        owners: owner.clone(),
    }));
    f.system
        .package_bootstrap
        .lock()
        .unwrap()
        .current
        .as_mut()
        .unwrap()
        .installer = Some((owner, binder));
    s.check(&["list", "staged-sessions"], 1, "");
    s.check(&["get-stagedsessions", "--only-ready"], 1, "");
    s.check(&["list", "staged-sessions", "--", "--bad"], 1, "");
    s.check(&["list", "staged-sessions", "ignored", "--bad"], 1, "");
    s.check(
        &["list", "staged-sessions", "--bad"],
        -1,
        "Error: Unknown option: --bad\n",
    );
}
#[test]
fn has_feature() {
    let s = Shell::new();
    s.check(&["has-feature", "fixture.feature", "2"], 0, "true\n");
    s.check(&["has-feature", "fixture.feature", "3"], 1, "false\n");
    assert_eq!(
        s.command(&["has-feature"]),
        (1, String::new(), "Error: expected FEATURE name\n".into())
    );
}

// The Java boundary is doubled here; golden error lines are emitted by the
// android-16.0.0_r1 DomainVerificationShell (runGetAppLinks/runGetAppLinkOwners).
struct DomainCold(Arc<LocalProcess>, Arc<std::sync::atomic::AtomicBool>);
impl Service for DomainCold {
    fn descriptor(&self) -> &str {
        aim_service_aidl::dev_aim_server_iapphostdomainverification::DESCRIPTOR
    }
    fn accepts_fds(&self) -> bool {
        true
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        use aim_service_aidl::dev_aim_server_iapphostdomainverification as domains;
        assert_eq!(call.code, domains::RUN_SHELL_COMMAND);
        assert_eq!((call.sender_pid, call.sender_euid), (97330, 2000));
        let a = domains::RunShellCommand::<Descriptor>::read(&mut call.data)?;
        assert_eq!(call.data.remaining(), 0);
        let file =
            |d: Descriptor| aim_binder_host::server::file_fd(&self.0.file(d.0).unwrap()).unwrap();
        let mut text = String::new();
        file(a.input.unwrap()).read_to_string(&mut text).unwrap();
        assert!(text.is_empty());
        let _output = file(a.output.unwrap());
        let mut error = file(a.error.unwrap());
        let args = a.arguments.unwrap();
        let (status, line) = match args[0].as_deref().unwrap() {
            "get-app-links" => (1, "Error: unknown option: --invalid"),
            "get-app-link-owners" => (1, "Error: no package name or domain specified"),
            "set-app-links-user-selection" | "set-app-links-allowed" => (0, ""),
            _ => panic!("unserved domain verb"),
        };
        assert_eq!(
            a.package_shell,
            self.1.load(std::sync::atomic::Ordering::Relaxed)
        );
        if !line.is_empty() {
            writeln!(error, "{line}").unwrap();
        }
        let mut reply = Parcel::new();
        domains::write_run_shell_command_reply(&mut reply, status);
        Ok(reply)
    }
}
#[test]
fn domain_shell_entrypoints_transfer_fds_identity_status_and_errors() {
    let mut shell = Shell::new();
    let original = &shell.session.fixture.original[0];
    let expected = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let frontend = original.add_service(Arc::new(DomainCold(original.clone(), expected.clone())));
    let mut p = Parcel::new();
    sp::AttachDomainVerification {
        frontend: Some(frontend),
    }
    .write(&mut p);
    lookup(original, "aim.system_process")
        .transact(sp::ATTACH_DOMAIN_VERIFICATION, &p, false)
        .unwrap()
        .reader()
        .read_exception()
        .unwrap()
        .unwrap();
    for package in [true, false] {
        expected.store(package, std::sync::atomic::Ordering::Relaxed);
        shell.package = lookup(
            &shell.process,
            if package {
                "package"
            } else {
                "domain_verification"
            },
        );
        for (verb, tail, expected) in [
            (
                "get-app-links",
                vec!["--invalid"],
                "Error: unknown option: --invalid\n",
            ),
            (
                "get-app-link-owners",
                vec![],
                "Error: no package name or domain specified\n",
            ),
        ] {
            let mut args = vec![verb];
            args.extend(tail);
            assert_eq!(shell.command(&args), (1, String::new(), expected.into()));
        }
        for args in [
            vec![
                "set-app-links-user-selection",
                "--user",
                "0",
                "--package",
                "p",
                "true",
                "all",
            ],
            vec![
                "set-app-links-allowed",
                "--user",
                "0",
                "--package",
                "p",
                "false",
            ],
        ] {
            assert_eq!(shell.command(&args), (0, String::new(), String::new()));
        }
    }
}
