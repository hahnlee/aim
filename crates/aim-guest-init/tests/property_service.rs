//! The property service on the host: shared-mapped areas, the futex wake,
//! the `property_service` socket protocol, `ctl.*` messages and socket fd
//! passing to a launched service.

mod common;

use std::collections::BTreeSet;
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::sync::atomic::AtomicU32;
use std::time::Duration;

use aim_android_init::props::protocol::{
    PROP_ERROR_HANDLE_CONTROL_MESSAGE, PROP_ERROR_INVALID_NAME, PROP_ERROR_READ_ONLY_PROPERTY,
    PROP_SUCCESS,
};
use aim_android_init::props::{PropAreaReader, PropertyInfoEntry, build_trie};
use aim_guest_init::futex::{WaitResult, wait_shared};
use aim_guest_init::props::mapped_properties;
use aim_guest_init::propsvc::client_set;
use aim_guest_init::{Boot, BootOptions, RunMode};

fn small_info() -> Vec<u8> {
    build_trie(
        &[
            PropertyInfoEntry::new("ro.", "u:object_r:ro_prop:s0", "string", false),
            PropertyInfoEntry::new("debug.", "u:object_r:debug_prop:s0", "string", false),
        ],
        "u:object_r:default_prop:s0",
        "string",
    )
    .unwrap()
}

/// A second, read-only `MAP_SHARED` mapping of an area file, as a guest
/// reader has it.
struct ReaderMapping {
    base: *mut libc::c_void,
    len: usize,
}

impl ReaderMapping {
    fn open(path: &std::path::Path) -> Self {
        let c_path = CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: mapping a file we created, read-only and shared.
        unsafe {
            let fd = libc::open(c_path.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC);
            assert!(fd >= 0);
            let len = libc::lseek(fd, 0, libc::SEEK_END) as usize;
            let base = libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ,
                libc::MAP_SHARED,
                fd,
                0,
            );
            libc::close(fd);
            assert_ne!(base, libc::MAP_FAILED);
            Self { base, len }
        }
    }

    fn bytes(&self) -> &[u8] {
        // SAFETY: mapped for len bytes.
        unsafe { std::slice::from_raw_parts(self.base.cast(), self.len) }
    }

    fn word(&self, offset: usize) -> &AtomicU32 {
        // SAFETY: aligned word inside the mapping.
        unsafe { &*((self.base as usize + offset) as *const AtomicU32) }
    }
}

impl Drop for ReaderMapping {
    fn drop(&mut self) {
        // SAFETY: our mapping.
        unsafe { libc::munmap(self.base, self.len) };
    }
}

// SAFETY: read-only shared memory.
unsafe impl Send for ReaderMapping {}
unsafe impl Sync for ReaderMapping {}

#[test]
fn mapped_areas_are_visible_and_wake_readers_across_mappings() {
    let root = common::temp_dir("props-mapped");
    let dir = root.join("__properties__");
    let mut service = mapped_properties(&dir, small_info()).unwrap();
    for file in [
        "property_info",
        "properties_serial",
        "u:object_r:debug_prop:s0",
    ] {
        let meta = std::fs::metadata(dir.join(file)).unwrap();
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(meta.permissions().mode() & 0o777, 0o444, "{file}");
    }
    assert!(service.init_set("debug.first", "1").is_success());
    let reader = ReaderMapping::open(&dir.join("u:object_r:debug_prop:s0"));
    let area = PropAreaReader::new(reader.bytes()).unwrap();
    assert_eq!(area.get("debug.first").unwrap().value, "1");

    // A reader sleeping on the global serial through its own mapping (a
    // different address) is woken by the writer's SHARED wake.
    let serial = std::sync::Arc::new(ReaderMapping::open(&dir.join("properties_serial")));
    let expected = serial.word(4).load(std::sync::atomic::Ordering::Acquire);
    let waiter = {
        let serial = serial.clone();
        std::thread::spawn(move || {
            let started = std::time::Instant::now();
            let result = wait_shared(serial.word(4), expected, 5_000_000_000);
            (result, started.elapsed())
        })
    };
    std::thread::sleep(Duration::from_millis(100));
    assert!(service.init_set("debug.second", "2").is_success());
    let (result, elapsed) = waiter.join().unwrap();
    assert!(
        matches!(result, WaitResult::Woken | WaitResult::ValueChanged),
        "{result:?}"
    );
    assert!(elapsed < Duration::from_secs(4), "{elapsed:?}");
    assert_ne!(
        serial.word(4).load(std::sync::atomic::Ordering::Acquire),
        expected
    );

    // An update wakes the prop_info serial too, and the reader sees it.
    service.init_set("debug.first", "changed");
    let area = PropAreaReader::new(reader.bytes()).unwrap();
    assert_eq!(area.get("debug.first").unwrap().value, "changed");
    drop(service);
    let _ = std::fs::remove_dir_all(&root);
}

fn run_boot(tag: &str, only: Option<&[&str]>) -> (Boot, std::path::PathBuf) {
    let root = common::temp_dir(tag);
    let image = common::fixture_image(&root);
    let mut options = BootOptions::new(image, root.join("data"), RunMode::Run);
    options.linux_run = Some(common::fake_linux_run(&root));
    options.only = only.map(|o| o.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>());
    (Boot::prepare(options).unwrap(), root)
}

/// Sends a set from a client thread and serves it on this thread.
fn set_over_socket(boot: &mut Boot, socket: usize, name: &str, value: &str) -> u32 {
    let path = boot.property_sockets()[socket].clone();
    let (name, value) = (name.to_string(), value.to_string());
    let client = std::thread::spawn(move || client_set(&path, &name, &value).unwrap());
    let mut served = false;
    for _ in 0..50 {
        if boot.serve_pending(Duration::from_millis(100)) {
            served = true;
            break;
        }
    }
    assert!(served, "request not received");
    client.join().unwrap()
}

#[test]
fn setprop2_over_the_socket_updates_the_areas() {
    let (mut boot, root) = run_boot("props-socket", None);
    let sockets = boot.property_sockets();
    assert!(sockets[0].ends_with("dev/socket/property_service"));
    assert!(sockets[1].ends_with("dev/socket/property_service_for_system"));
    assert_eq!(
        boot.property("ro.property_service.version").as_deref(),
        Some("2")
    );
    assert_eq!(
        set_over_socket(&mut boot, 0, "debug.guest", "yes"),
        PROP_SUCCESS
    );
    assert_eq!(boot.property("debug.guest").as_deref(), Some("yes"));
    // The same value is in the file the guest maps.
    let props_dir = boot.layout.properties_dir();
    let info = boot.properties();
    let context = {
        let props = info.borrow();
        let (context, _) = props.areas().info_area().property_info("debug.guest");
        context.unwrap().to_string()
    };
    let bytes = std::fs::read(props_dir.join(&context)).unwrap();
    assert_eq!(
        PropAreaReader::new(&bytes)
            .unwrap()
            .get("debug.guest")
            .unwrap()
            .value,
        "yes"
    );
    assert_eq!(
        set_over_socket(&mut boot, 1, "ro.build.id", "other"),
        PROP_ERROR_READ_ONLY_PROPERTY
    );
    assert_eq!(
        set_over_socket(&mut boot, 0, "bad..name", "1"),
        PROP_ERROR_INVALID_NAME
    );

    // persist.* is written once persistent properties are loaded.
    boot.properties().borrow_mut().persistent_properties_loaded = true;
    assert_eq!(
        set_over_socket(&mut boot, 0, "persist.test.value", "42"),
        PROP_SUCCESS
    );
    let persisted = std::fs::read(root.join("data/data/property/persistent_properties")).unwrap();
    let decoded = aim_guest_init::props::decode_persistent_properties(&persisted).unwrap();
    assert!(decoded.contains(&("persist.test.value".to_string(), "42".to_string())));
    drop(boot);
    common::make_writable(&root);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn ctl_messages_start_and_stop_services_with_passed_socket_fds() {
    let (mut boot, root) = run_boot("props-ctl", None);
    // logd is declared by the fixture's logd.rc with three sockets and two
    // files.
    assert_eq!(
        set_over_socket(&mut boot, 0, "ctl.start", "logd"),
        PROP_SUCCESS
    );
    assert_eq!(boot.property("init.svc.logd").as_deref(), Some("running"));
    let record = boot.executor.supervisor.record("logd").unwrap();
    let pid = record.pid.unwrap();
    let identity_file = boot.layout.identity_dir().join("logd.1");
    let probe = common::wait_for_file(&identity_file.with_extension("1.probe"));
    assert!(probe.contains("program=/system/bin/logd"), "{probe}");
    assert!(probe.contains("ANDROID_SOCKET_logd=3"), "{probe}");
    assert!(probe.contains("ANDROID_SOCKET_logdr=4"), "{probe}");
    assert!(probe.contains("ANDROID_SOCKET_logdw=5"), "{probe}");
    for fd in [3, 4, 5] {
        assert!(
            probe.contains(&format!("fd{fd}=socket")),
            "fd {fd}: {probe}"
        );
    }
    // /dev/kmsg is the runtime's log file; /proc/kmsg does not exist, so
    // its variable is not published.
    assert!(probe.contains("ANDROID_FILE__dev_kmsg=7"), "{probe}");
    assert!(probe.contains("fd7=open"), "{probe}");
    assert!(!probe.contains("ANDROID_FILE__proc_kmsg"), "{probe}");
    let identity = std::fs::read_to_string(&identity_file).unwrap();
    assert!(identity.contains("uid\t1036\n"), "{identity}");
    assert!(identity.contains("groups\t1000 1032 3009\n"), "{identity}");
    let by_pid = boot
        .layout
        .identity_dir()
        .join("by-pid")
        .join(pid.to_string());
    assert!(by_pid.exists());
    // The sockets exist at the mapped /dev/socket with init's modes.
    use std::os::unix::fs::{FileTypeExt, PermissionsExt};
    let logdw = std::fs::metadata(boot.layout.socket_dir().join("logdw")).unwrap();
    assert!(logdw.file_type().is_socket());
    assert_eq!(logdw.permissions().mode() & 0o777, 0o222);
    let attrs = std::fs::read_to_string(boot.layout.fs_attrs_file()).unwrap();
    assert!(
        attrs.contains("/dev/socket/logd\t1036\t1036\t666\n"),
        "{attrs}"
    );
    let table = std::fs::read_to_string(boot.layout.sockets_file()).unwrap();
    assert!(
        table.contains("logd\t/dev/socket/logdr\tseqpacket\t"),
        "{table}"
    );

    // ctl.stop kills the process group; the reap publishes "stopped".
    assert_eq!(
        set_over_socket(&mut boot, 0, "ctl.stop", "logd"),
        PROP_SUCCESS
    );
    assert_eq!(boot.property("init.svc.logd").as_deref(), Some("stopping"));
    for _ in 0..200 {
        boot.executor.poll_processes();
        if boot
            .executor
            .supervisor
            .record("logd")
            .unwrap()
            .pid
            .is_none()
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    boot.executor.drain_supervisor();
    assert_eq!(boot.property("init.svc.logd").as_deref(), Some("stopped"));
    // Sockets are removed when the service is reaped.
    assert!(!boot.layout.socket_dir().join("logdw").exists());

    // Unknown control actions fail with the control-message error.
    assert_eq!(
        set_over_socket(&mut boot, 0, "ctl.frobnicate", "logd"),
        PROP_ERROR_HANDLE_CONTROL_MESSAGE
    );
    // Starting an undeclared service is a logged no-op.
    assert_eq!(
        set_over_socket(&mut boot, 0, "ctl.start", "no.such.hal"),
        PROP_SUCCESS
    );
    // oneshot_on sets the flag.
    assert_eq!(
        set_over_socket(&mut boot, 0, "ctl.oneshot_on", "logd"),
        PROP_SUCCESS
    );
    let flags = boot.executor.supervisor.record("logd").unwrap().flags;
    assert!(flags & aim_guest_init::supervisor::flags::ONESHOT != 0);
    drop(boot);
    common::make_writable(&root);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn only_filter_skips_other_services() {
    let (mut boot, root) = run_boot("props-only", Some(&["servicemanager"]));
    assert_eq!(
        set_over_socket(&mut boot, 0, "ctl.start", "logd"),
        PROP_SUCCESS
    );
    assert!(
        boot.executor
            .supervisor
            .record("logd")
            .unwrap()
            .pid
            .is_none()
    );
    assert_eq!(
        set_over_socket(&mut boot, 0, "ctl.start", "servicemanager"),
        PROP_SUCCESS
    );
    assert!(
        boot.executor
            .supervisor
            .record("servicemanager")
            .unwrap()
            .pid
            .is_some()
    );
    drop(boot);
    common::make_writable(&root);
    let _ = std::fs::remove_dir_all(&root);
}
