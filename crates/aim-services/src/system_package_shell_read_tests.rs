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
    io::{Read, Seek, SeekFrom},
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
struct Cold(Arc<LocalProcess>);
impl Service for Cold {
    fn descriptor(&self) -> &str {
        cold::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
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
        context.system.system_permissions = Some(
            [(
                2000,
                [
                    "android.permission.MANAGE_USERS".into(),
                    "android.permission.INTERACT_ACROSS_USERS_FULL".into(),
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
        let node = original.add_service(Arc::new(Cold(original.clone())));
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
