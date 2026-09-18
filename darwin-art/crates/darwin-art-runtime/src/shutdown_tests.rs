use crate::{NativeResource, RuntimeError, RuntimePhase, RuntimeSession, Subsystem};
use std::cell::RefCell;
use std::env;
use std::fs;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

const NOT_READY: i32 = darwin_art_engine_sys::PROCESS_SHUTDOWN_NOT_READY;

type Events = Rc<RefCell<Vec<String>>>;
const CHILD_MODE_ENV: &str = "DARWIN_ART_RUNTIME_DROP_CHILD";
const MARKER_ENV: &str = "DARWIN_ART_RUNTIME_DROP_MARKER";

#[derive(Debug)]
struct EngineProbe {
    statuses: Vec<i32>,
    calls: usize,
    events: Events,
    drop_marker: Option<PathBuf>,
}

impl NativeResource for EngineProbe {
    fn close(&mut self) -> i32 {
        let call = self.calls + 1;
        self.calls = call;
        let status = self.statuses.get(call - 1).copied().unwrap_or(0);
        let event = match status {
            NOT_READY => format!("engine-pending-{call}"),
            0 => "engine-terminal".to_owned(),
            status => format!("engine-error-{status}"),
        };
        self.events.borrow_mut().push(event);
        status
    }
}

impl Drop for EngineProbe {
    fn drop(&mut self) {
        self.events.borrow_mut().push("drop-engine".to_owned());
        write_drop_marker(self.drop_marker.as_deref());
    }
}

#[derive(Debug)]
struct ResourceProbe {
    name: &'static str,
    events: Events,
    drop_marker: Option<PathBuf>,
}

impl NativeResource for ResourceProbe {
    fn close(&mut self) -> i32 {
        self.events
            .borrow_mut()
            .push(format!("{}-close", self.name));
        0
    }

    fn finalize(&mut self) -> i32 {
        self.events
            .borrow_mut()
            .push(format!("finalize-{}", self.name));
        0
    }

    fn clear(&mut self) -> i32 {
        self.events
            .borrow_mut()
            .push(format!("{}-clear", self.name));
        0
    }
}

impl Drop for ResourceProbe {
    fn drop(&mut self) {
        self.events.borrow_mut().push(format!("drop-{}", self.name));
        write_drop_marker(self.drop_marker.as_deref());
    }
}

fn resource(name: &'static str, events: &Events) -> ResourceProbe {
    resource_with_marker(name, events, None)
}

fn resource_with_marker(
    name: &'static str,
    events: &Events,
    drop_marker: Option<PathBuf>,
) -> ResourceProbe {
    ResourceProbe {
        name,
        events: Rc::clone(events),
        drop_marker,
    }
}

fn write_drop_marker(path: Option<&Path>) {
    if let Some(path) = path {
        fs::write(path, b"dropped").expect("drop marker must be writable");
    }
}

fn running_session(
    statuses: Vec<i32>,
) -> (
    RuntimeSession<EngineProbe, ResourceProbe, ResourceProbe, ResourceProbe>,
    Events,
) {
    let events = Rc::new(RefCell::new(Vec::new()));
    let mut session = RuntimeSession::new();
    session.start().unwrap();
    session
        .attach_engine(EngineProbe {
            statuses,
            calls: 0,
            events: Rc::clone(&events),
            drop_marker: None,
        })
        .unwrap();
    session
        .attach_provider(resource("provider", &events))
        .unwrap();
    session
        .attach_surface(resource("surface", &events))
        .unwrap();
    session
        .attach_graphics(resource("graphics", &events))
        .unwrap();
    session.install_subsystem(Subsystem::Engine).unwrap();
    session.install_subsystem(Subsystem::ElfNamespace).unwrap();
    session.install_subsystem(Subsystem::Graphics).unwrap();
    session.install_subsystem(Subsystem::Surface).unwrap();
    session.mark_running().unwrap();
    (session, events)
}

fn unresolved_session(
    marker: PathBuf,
    failed: bool,
) -> RuntimeSession<EngineProbe, ResourceProbe, ResourceProbe, ResourceProbe> {
    let events = Rc::new(RefCell::new(Vec::new()));
    let mut session = RuntimeSession::new();
    session.start().unwrap();
    session
        .attach_engine(EngineProbe {
            statuses: Vec::new(),
            calls: 0,
            events: Rc::clone(&events),
            drop_marker: Some(marker.clone()),
        })
        .unwrap();
    session
        .attach_provider(resource_with_marker(
            "provider",
            &events,
            Some(marker.clone()),
        ))
        .unwrap();
    session
        .attach_surface(resource_with_marker(
            "surface",
            &events,
            Some(marker.clone()),
        ))
        .unwrap();
    session
        .attach_graphics(resource_with_marker("graphics", &events, Some(marker)))
        .unwrap();
    session.install_subsystem(Subsystem::Engine).unwrap();
    session.install_subsystem(Subsystem::ElfNamespace).unwrap();
    session.install_subsystem(Subsystem::Graphics).unwrap();
    session.install_subsystem(Subsystem::Surface).unwrap();
    session.mark_running().unwrap();
    if failed {
        session.fail(RuntimeError::AlreadyFailed);
    } else {
        session.begin_shutdown().unwrap();
    }
    session
}

fn run_drop_child(mode: &str) -> bool {
    if env::var(CHILD_MODE_ENV).ok().as_deref() != Some(mode) {
        return false;
    }
    let marker = PathBuf::from(env::var_os(MARKER_ENV).expect("child marker path"));
    let session = unresolved_session(marker, mode == "failed");
    drop(session);
    unreachable!("unresolved runtime drop must abort");
}

fn assert_drop_aborts(mode: &str, test_name: &str) {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_nanos();
    let directory = env::temp_dir().join(format!(
        "darwin-art-runtime-drop-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir(&directory).expect("create owned shutdown test directory");
    let marker = directory.join("drop-marker");
    let status = Command::new(env::current_exe().expect("current test binary"))
        .arg("--exact")
        .arg(test_name)
        .arg("--nocapture")
        .env(CHILD_MODE_ENV, mode)
        .env(MARKER_ENV, &marker)
        .status()
        .expect("run isolated shutdown child");
    assert_eq!(status.signal(), Some(6), "child did not abort: {status:?}");
    assert!(
        !marker.exists(),
        "resource Drop ran before fail-closed abort"
    );
    fs::remove_dir(&directory).expect("remove owned shutdown test directory");
}

#[test]
fn readiness_retry_keeps_teardown_owners_until_terminal_engine_success() {
    let (mut session, events) = running_session(vec![NOT_READY, NOT_READY, 0]);

    assert_eq!(session.shutdown_native(), Ok(()));
    assert_eq!(session.phase(), RuntimePhase::Stopped);
    assert!(session.is_empty());

    let observed = events.borrow().clone();
    assert_eq!(
        observed,
        vec![
            "graphics-close",
            "engine-pending-1",
            "engine-pending-2",
            "engine-terminal",
            "surface-close",
            "drop-surface",
            "finalize-graphics",
            "drop-graphics",
            "provider-clear",
            "drop-provider",
            "drop-engine",
        ]
    );
    assert_eq!(
        observed
            .iter()
            .filter(|event| event.as_str() == "graphics-close")
            .count(),
        1
    );
    let terminal = observed
        .iter()
        .position(|event| event == "engine-terminal")
        .unwrap();
    for event in [
        "surface-close",
        "drop-surface",
        "finalize-graphics",
        "drop-graphics",
        "provider-clear",
        "drop-provider",
        "drop-engine",
    ] {
        assert!(
            observed
                .iter()
                .position(|candidate| candidate == event)
                .unwrap()
                > terminal,
            "{event} ran before the engine reached terminal readiness"
        );
    }
}

#[test]
fn terminal_engine_error_retains_all_owners_and_subsystem_leases() {
    let (mut session, events) = running_session(vec![NOT_READY, -23]);
    // Inert mock resources deliberately retain the failed owner obligation;
    // destroying an unresolved real session must fail closed before any Drop.
    let mut session = std::mem::ManuallyDrop::new(session);

    assert_eq!(
        session.shutdown_native(),
        Err(RuntimeError::EngineFailure { status: -23 })
    );
    assert_eq!(session.phase(), RuntimePhase::ShuttingDown);
    assert!(!session.is_empty());
    for subsystem in [
        Subsystem::Engine,
        Subsystem::ElfNamespace,
        Subsystem::Graphics,
        Subsystem::Surface,
    ] {
        assert!(session.subsystem_active(subsystem));
    }
    assert!(session.engine().is_some());
    assert!(session.provider().is_some());
    assert!(session.surface().is_some());
    assert!(session.graphics().is_some());

    assert_eq!(
        &*events.borrow(),
        &["graphics-close", "engine-pending-1", "engine-error--23"]
    );
}

#[test]
fn unresolved_shutting_down_drop_aborts_before_resource_drop() {
    if run_drop_child("shutting-down") {
        return;
    }
    assert_drop_aborts(
        "shutting-down",
        "shutdown_tests::unresolved_shutting_down_drop_aborts_before_resource_drop",
    );
}

#[test]
fn unresolved_failed_drop_aborts_before_resource_drop() {
    if run_drop_child("failed") {
        return;
    }
    assert_drop_aborts(
        "failed",
        "shutdown_tests::unresolved_failed_drop_aborts_before_resource_drop",
    );
}
