//! The service state machine and restart policies, over the dry-run
//! launcher (synthetic pids, exits injected by the test).

mod common;

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use darwin_android_init::engine::ExecSpec;
use darwin_android_init::rc::{IdResolver, ParseEnv, Parser, SectionKind, Service};
use darwin_guest_init::launch::{DryRunLauncher, Exit, Launcher, LinuxRun, LinuxRunOptions};
use darwin_guest_init::paths::Layout;
use darwin_guest_init::supervisor::{
    DEFAULT_PATH, Planner, StartOutcome, Supervisor, SupervisorEvent, flags,
};

const RC: &str = r#"
service svc /system/bin/testsvc --flag ${ro.test.arg}
    class main
    restart_period 2
    onrestart setprop svc.restarted 1
    onrestart restart other

service other /system/bin/testsvc
    class main

service one /system/bin/testsvc
    oneshot
    disabled

service crit /system/bin/testsvc
    critical window=1

service sock /system/bin/testsvc
    class core
    socket foo stream 0660 system system
    socket bar dgram+passcred 0666
    file /dev/kmsg w
    setenv FOO bar
    user system
    group system log
    capabilities NET_ADMIN
    rlimit nofile 1024 4096

service timed /system/bin/testsvc
    timeout_period 3
    disabled

service missing /system/bin/does-not-exist
    class main
"#;

struct Fixture {
    supervisor: Supervisor,
    launcher: DryRunLauncher,
    planner: Planner,
    props: BTreeMap<String, String>,
    now: Instant,
    _root: std::path::PathBuf,
}

fn parse(ids: &IdResolver, props: &BTreeMap<String, String>) -> Vec<Service> {
    let mut parser = Parser::new(
        ParseEnv {
            properties: props,
            vendor_api_level: 36,
            ids,
        },
        SectionKind::Boot,
    );
    parser.parse_data("/system/etc/init/test.rc", RC.as_bytes());
    let parsed = parser.finish();
    assert_eq!(parsed.parse_error_count, 0, "{:?}", parsed.diagnostics);
    parsed.services
}

fn fixture(tag: &str) -> Fixture {
    let root = common::temp_dir(tag);
    let image = common::fixture_image(&root);
    let layout = Layout::new(image.clone(), root.join("data"), None);
    layout.prepare().unwrap();
    let ids = IdResolver::builtin();
    let mut props = BTreeMap::new();
    props.insert("ro.test.arg".to_string(), "expanded".to_string());
    let services = parse(&ids, &props);
    let mut supervisor = Supervisor::new();
    supervisor.sync(&services);
    let linux_run = LinuxRun {
        binary: "/opt/linux-run".into(),
        image,
        path_map_file: layout.path_map_file(),
        binder: None,
        trace: false,
        options: LinuxRunOptions::CONTRACT,
    };
    let now = Instant::now();
    Fixture {
        supervisor,
        launcher: DryRunLauncher::new(linux_run),
        planner: Planner {
            map: layout.path_map(),
            layout,
            ids,
            vendor_api_level: 36,
            env: vec![("PATH".into(), DEFAULT_PATH.into())],
            rlimits: Vec::new(),
            boot_epoch: now,
        },
        props,
        now,
        _root: root,
    }
}

impl Fixture {
    fn start(&mut self, name: &str) -> Result<StartOutcome, String> {
        self.supervisor.start(
            name,
            &mut self.launcher,
            &self.planner,
            &self.props,
            self.now,
        )
    }

    fn reap(&mut self, pid: u32, exit: Exit) {
        assert!(self.supervisor.reap(
            pid,
            exit,
            &mut self.launcher,
            &self.planner,
            &self.props,
            self.now
        ));
    }

    fn pid(&self, name: &str) -> u32 {
        self.supervisor.record(name).unwrap().pid.unwrap()
    }

    fn flags(&self, name: &str) -> u32 {
        self.supervisor.record(name).unwrap().flags
    }

    /// Applies property events like the executor; returns all events.
    fn events(&mut self) -> Vec<SupervisorEvent> {
        let events = self.supervisor.take_events();
        for event in &events {
            if let SupervisorEvent::SetProperty { name, value } = event {
                self.props.insert(name.clone(), value.clone());
            }
        }
        events
    }
}

#[test]
fn start_and_stop_publish_init_svc() {
    let mut f = fixture("sv-start");
    let StartOutcome::Started { pid, spec } = f.start("svc").unwrap() else {
        panic!()
    };
    assert_eq!(spec.argv, ["/system/bin/testsvc", "--flag", "expanded"]);
    f.events();
    assert_eq!(f.props["init.svc.svc"], "running");
    assert_eq!(f.props["init.svc_debug_pid.svc"], pid.to_string());
    assert!(f.props.contains_key("ro.boottime.svc"));
    assert_eq!(f.start("svc").unwrap(), StartOutcome::AlreadyRunning);

    f.supervisor
        .stop("svc", &mut f.launcher, &f.planner)
        .unwrap();
    f.events();
    assert_eq!(f.props["init.svc.svc"], "stopping");
    assert_eq!(f.launcher.kills.last(), Some(&(pid, libc::SIGKILL)));
    for (pid, exit) in f.launcher.reap() {
        f.reap(pid, exit);
    }
    let events = f.events();
    assert_eq!(f.props["init.svc.svc"], "stopped");
    assert_eq!(f.props["init.svc_debug_pid.svc"], "");
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, SupervisorEvent::OnRestart { .. }))
    );
    assert!(f.flags("svc") & flags::DISABLED != 0);
    let (due, _) = f
        .supervisor
        .process_actions(&mut f.launcher, f.now + Duration::from_secs(60));
    assert!(due.is_empty());
}

#[test]
fn crash_restarts_after_restart_period_and_runs_onrestart() {
    let mut f = fixture("sv-crash");
    f.start("svc").unwrap();
    let pid = f.pid("svc");
    f.events();
    f.reap(pid, Exit::Signal(libc::SIGSEGV));
    let events = f.events();
    assert_eq!(f.props["init.svc.svc"], "restarting");
    let onrestart = events
        .iter()
        .find_map(|e| match e {
            SupervisorEvent::OnRestart { commands, .. } => Some(commands.clone()),
            _ => None,
        })
        .expect("onrestart");
    assert_eq!(onrestart.len(), 2);
    assert_eq!(onrestart[0].command_string(), "setprop svc.restarted 1");
    assert!(f.flags("svc") & flags::RESTARTING != 0);

    // restart_period counts from the start time.
    let (due, next) = f
        .supervisor
        .process_actions(&mut f.launcher, f.now + Duration::from_secs(1));
    assert!(due.is_empty());
    assert_eq!(next, Some(f.now + Duration::from_secs(2)));
    let (due, _) = f
        .supervisor
        .process_actions(&mut f.launcher, f.now + Duration::from_millis(2001));
    assert_eq!(due, ["svc"]);
    f.now += Duration::from_millis(2001);
    let StartOutcome::Started { pid: second, .. } = f.start("svc").unwrap() else {
        panic!()
    };
    assert_ne!(second, pid);
    assert_eq!(f.supervisor.record("svc").unwrap().starts, 2);
    f.events();
    assert_eq!(f.props["init.svc.svc"], "running");
}

#[test]
fn oneshot_services_stay_down_and_restart_revives_them() {
    let mut f = fixture("sv-oneshot");
    assert!(f.flags("one") & flags::DISABLED != 0);
    assert!(f.flags("one") & flags::RC_DISABLED != 0);
    f.start("one").unwrap();
    let pid = f.pid("one");
    f.reap(pid, Exit::Code(0));
    f.events();
    assert_eq!(f.props["init.svc.one"], "stopped");
    assert!(f.flags("one") & flags::DISABLED != 0);
    let (due, _) = f
        .supervisor
        .process_actions(&mut f.launcher, f.now + Duration::from_secs(60));
    assert!(due.is_empty());

    // restart of a running oneshot: stop now, start again on exit.
    f.start("one").unwrap();
    let pid = f.pid("one");
    let outcome = f
        .supervisor
        .restart("one", &mut f.launcher, &f.planner, &f.props, f.now)
        .unwrap();
    assert!(outcome.is_none());
    assert!(f.flags("one") & flags::RESTART != 0);
    f.reap(pid, Exit::Signal(libc::SIGKILL));
    assert!(f.flags("one") & flags::RESTARTING != 0);
    assert!(f.flags("one") & flags::DISABLED == 0);
    let (due, _) = f
        .supervisor
        .process_actions(&mut f.launcher, f.now + Duration::from_secs(6));
    assert_eq!(due, ["one"]);
}

#[test]
fn critical_service_crashing_five_times_is_fatal() {
    let mut f = fixture("sv-critical");
    for crash in 1..=5 {
        f.start("crit").unwrap();
        let pid = f.pid("crit");
        f.reap(pid, Exit::Code(1));
        let fatal = f
            .events()
            .into_iter()
            .any(|e| matches!(e, SupervisorEvent::Fatal(_)));
        assert_eq!(fatal, crash == 5, "crash {crash}");
    }
    // A clean exit does not count.
    let mut f = fixture("sv-critical-ok");
    for _ in 0..6 {
        f.start("crit").unwrap();
        let pid = f.pid("crit");
        f.reap(pid, Exit::Code(0));
        assert!(
            !f.events()
                .iter()
                .any(|e| matches!(e, SupervisorEvent::Fatal(_)))
        );
    }
}

#[test]
fn class_start_respects_disabled_and_enable_starts_later() {
    let mut f = fixture("sv-enable");
    let started = f
        .supervisor
        .start_if_not_disabled("one", &mut f.launcher, &f.planner, &f.props, f.now)
        .unwrap();
    assert!(started.is_none());
    assert!(f.flags("one") & flags::DISABLED_START != 0);
    let started = f
        .supervisor
        .enable("one", &mut f.launcher, &f.planner, &f.props, f.now)
        .unwrap();
    assert!(matches!(started, Some(StartOutcome::Started { .. })));
    assert_eq!(
        f.supervisor.class_members("main"),
        ["missing", "other", "svc"]
    );
    // A missing program disables the service, like init.
    let error = f.start("missing").unwrap_err();
    assert!(
        error.contains("Cannot find '/system/bin/does-not-exist'"),
        "{error}"
    );
    assert!(f.flags("missing") & flags::DISABLED != 0);
}

#[test]
fn reset_keeps_rc_disabled_services_disabled() {
    let mut f = fixture("sv-reset");
    f.start("one").unwrap();
    let pid = f.pid("one");
    f.supervisor
        .reset("one", &mut f.launcher, &f.planner)
        .unwrap();
    assert!(f.flags("one") & flags::DISABLED != 0);
    f.reap(pid, Exit::Signal(libc::SIGKILL));
    f.start("other").unwrap();
    let pid = f.pid("other");
    f.supervisor
        .reset("other", &mut f.launcher, &f.planner)
        .unwrap();
    assert!(f.flags("other") & flags::RESET != 0);
    f.reap(pid, Exit::Signal(libc::SIGKILL));
    f.events();
    assert_eq!(f.props["init.svc.other"], "stopped");
}

#[test]
fn exec_services_are_temporary_and_unblock_on_exit() {
    let mut f = fixture("sv-exec");
    let spec = ExecSpec {
        seclabel: None,
        user: Some("system".into()),
        group: Some("system".into()),
        supplementary_groups: vec!["log".into()],
        args: vec!["/system/bin/testsvc".into(), "arg".into()],
    };
    let name = f
        .supervisor
        .add_exec_service(&spec, &f.planner.ids)
        .unwrap();
    assert_eq!(name, "exec 1 (/system/bin/testsvc arg)");
    let StartOutcome::Started { pid, spec } = f
        .supervisor
        .exec_start(&name, &mut f.launcher, &f.planner, &f.props, f.now)
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(spec.identity.uid, 1000);
    assert_eq!(spec.identity.groups, [1007]);
    // Temporary services publish no init.svc property.
    assert!(f.events().is_empty());
    f.reap(pid, Exit::Code(0));
    let events = f.events();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, SupervisorEvent::ExecFinished { .. }))
    );
    assert!(!f.supervisor.contains(&name));

    // exec_start of a declared service.
    f.start("other").unwrap();
    let other = f.pid("other");
    f.reap(other, Exit::Code(0));
    f.events();
    f.now += Duration::from_secs(10);
    let StartOutcome::Started { pid, .. } = f
        .supervisor
        .exec_start("one", &mut f.launcher, &f.planner, &f.props, f.now)
        .unwrap()
    else {
        panic!()
    };
    assert!(f.flags("one") & flags::EXEC != 0);
    f.reap(pid, Exit::Code(1));
    assert!(
        f.events()
            .iter()
            .any(|e| matches!(e, SupervisorEvent::ExecFinished { service } if service == "one"))
    );
    assert!(f.flags("one") & flags::EXEC == 0);
}

#[test]
fn launch_spec_carries_descriptors_environment_and_identity() {
    let mut f = fixture("sv-spec");
    let StartOutcome::Started { spec, .. } = f.start("sock").unwrap() else {
        panic!()
    };
    let env: BTreeMap<_, _> = spec.env.iter().cloned().collect();
    assert_eq!(env["ANDROID_SOCKET_foo"], "3");
    assert_eq!(env["ANDROID_SOCKET_bar"], "4");
    assert_eq!(env["ANDROID_FILE__dev_kmsg"], "5");
    assert_eq!(env["FOO"], "bar");
    assert_eq!(env["PATH"], DEFAULT_PATH);
    assert_eq!(spec.sockets[0].perm, 0o660);
    assert_eq!((spec.sockets[0].uid, spec.sockets[0].gid), (1000, 1000));
    assert!(spec.sockets[1].passcred);
    assert_eq!(
        spec.sockets[0].host_path,
        f.planner.layout.socket_dir().join("foo")
    );
    assert_eq!(spec.files[0].host, f.planner.layout.dev_dir().join("kmsg"));
    let identity = &spec.identity;
    assert_eq!((identity.uid, identity.gid), (1000, 1000));
    assert_eq!(identity.groups, [1007]);
    let net_admin = 1u64 << 12;
    assert_eq!(identity.capabilities.effective, net_admin);
    assert_eq!(identity.capabilities.ambient, net_admin);
    assert_eq!(identity.rlimits.len(), 1);
    assert_eq!(identity.rlimits[0].resource, 7);
    let command = f.launcher.linux_run.command_line(&spec);
    assert_eq!(
        command[..3],
        [
            "/opt/linux-run".to_string(),
            "--root".into(),
            f.planner.layout.image.display().to_string()
        ]
    );
    assert!(command.contains(&"--inherit-env".to_string()));
    assert_eq!(command.last().unwrap(), "/system/bin/testsvc");
    // No binder host named, and an empty label is init's to compute.
    assert!(!command.contains(&"--binder".to_string()));
    assert!(!command.contains(&"--seclabel".to_string()));

    let mut linux_run = f.launcher.linux_run.clone();
    linux_run.binder = Some("dev.test.binder".into());
    let mut labelled = spec.clone();
    labelled.identity.seclabel = "u:r:test:s0".into();
    let command = linux_run.command_line(&labelled);
    let value = |flag: &str| {
        let at = command.iter().position(|a| a == flag)?;
        Some(command[at + 1].clone())
    };
    assert_eq!(value("--binder").as_deref(), Some("dev.test.binder"));
    assert_eq!(value("--seclabel").as_deref(), Some("u:r:test:s0"));
    assert_eq!(command.last().unwrap(), "/system/bin/testsvc");
}

#[test]
fn timeout_period_kills_the_service() {
    let mut f = fixture("sv-timeout");
    f.start("timed").unwrap();
    let pid = f.pid("timed");
    let (_, next) = f
        .supervisor
        .process_actions(&mut f.launcher, f.now + Duration::from_secs(1));
    assert_eq!(next, Some(f.now + Duration::from_secs(3)));
    f.supervisor
        .process_actions(&mut f.launcher, f.now + Duration::from_secs(4));
    assert_eq!(f.launcher.kills.last(), Some(&(pid, libc::SIGKILL)));
}
