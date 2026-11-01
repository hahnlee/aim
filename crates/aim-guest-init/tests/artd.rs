//! Lazy ART daemon admission through init, with synthetic processes only.

mod common;

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::time::Instant;

use aim_android_init::engine::Command;
use aim_android_init::props::build_trie;
use aim_android_init::props::service::ControlMessage;
use aim_android_init::rc::{IdResolver, ParseEnv, Parser, SectionKind};
use aim_guest_init::executor::GuestExecutor;
use aim_guest_init::fsops::{Effect, FsOps};
use aim_guest_init::identity::Capabilities;
use aim_guest_init::launch::{DryRunLauncher, Exit, LinuxRun, LinuxRunOptions};
use aim_guest_init::paths::Layout;
use aim_guest_init::props::heap_properties;
use aim_guest_init::supervisor::{Planner, flags};

// Unmodified art/build/apex/art.rc, android-16.0.0_r1, installed as etc/init.rc.
const ART_RC: &[u8] = include_bytes!("fixtures/art.rc");
const SERVICES: &str = include_str!("../../../image/app-host/services");

fn allowed() -> BTreeSet<String> {
    SERVICES
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_string)
        .collect()
}

fn fixture(tag: &str) -> (GuestExecutor, std::path::PathBuf) {
    let root = common::temp_dir(tag);
    let image = common::fixture_image(&root);
    let binary = image.join("apex/com.android.art/bin/artd");
    std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
    std::fs::write(binary, b"synthetic launcher input").unwrap();
    let layout = Layout::new(image.clone(), root.join("data"), None);
    layout.prepare().unwrap();
    let ids = IdResolver::builtin();
    let properties = BTreeMap::<String, String>::new();
    let mut parser = Parser::new(
        ParseEnv {
            properties: &properties,
            vendor_api_level: 36,
            ids: &ids,
        },
        SectionKind::Apex,
    );
    parser.parse_data("/apex/com.android.art/etc/init.rc", ART_RC);
    let parsed = parser.finish();
    assert_eq!(parsed.parse_error_count, 0, "{:?}", parsed.diagnostics);
    let info = build_trie(&[], "u:object_r:default_prop:s0", "string").unwrap();
    let props = Rc::new(RefCell::new(heap_properties(info).unwrap()));
    let launcher = DryRunLauncher::new(LinuxRun {
        binary: "linux-run".into(),
        image,
        path_map_file: layout.path_map_file(),
        binder: Some("fixture.binder".into()),
        gpu: None,
        vulkan: None,
        display: None,
        trace: false,
        options: LinuxRunOptions::CONTRACT,
    });
    let planner = Planner {
        map: layout.path_map(),
        layout: layout.clone(),
        ids,
        vendor_api_level: 36,
        env: vec![],
        rlimits: vec![],
        boot_epoch: Instant::now(),
    };
    let fs = FsOps::new(layout.path_map(), layout.fs_attrs_file(), false);
    let mut executor = GuestExecutor::new(props, Box::new(launcher), planner, fs, Some(allowed()));
    executor.sync_services(&parsed.services);
    (executor, root)
}

fn request(executor: &mut GuestExecutor, interface: &str) -> Effect {
    executor
        .handle_control(&ControlMessage {
            action: "interface_start".into(),
            target: interface.into(),
            from_pid: 1,
        })
        .unwrap()
}

fn core_start(executor: &mut GuestExecutor) {
    executor
        .run_command(&Command::ClassStart {
            class: "core".into(),
        })
        .unwrap();
}

#[test]
fn fixture_matches_pinned_original_art_init() {
    let Some(path) = aim_paths::input(
        aim_paths::original_image().join("apex/com.android.art/etc/init.rc"),
        "image",
    ) else {
        return;
    };
    assert_eq!(std::fs::read(path).unwrap(), ART_RC);
}

#[test]
fn original_artd_spec_is_preserved_without_eager_start() {
    let (mut executor, root) = fixture("artd-spec");
    let def = &executor.supervisor.record("artd").unwrap().def;
    assert_eq!(def.args, ["/apex/com.android.art/bin/artd"]);
    assert!(def.disabled && def.oneshot);
    assert_eq!(def.classnames, BTreeSet::from(["core".into()]));
    assert_eq!(def.interfaces, BTreeSet::from(["aidl/artd".into()]));
    assert_eq!(def.user.as_ref().unwrap().id, 1082);
    assert_eq!(def.group.as_ref().unwrap().id, 1082);
    assert!(def.supplementary_groups.is_empty());
    assert_eq!(def.capabilities, Some(0xf));
    core_start(&mut executor);
    executor.poll_processes();
    assert!(executor.launches.is_empty());
    assert!(
        executor
            .props
            .borrow()
            .areas()
            .get("init.svc.artd")
            .is_none()
    );
    drop(executor);
    common::make_writable(&root);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn interface_start_obeys_allowlist_and_exclusion() {
    let (mut executor, root) = fixture("artd-filter");
    executor.only.as_mut().unwrap().remove("artd");
    assert_eq!(
        request(&mut executor, "aidl/artd"),
        Effect::Skipped("'artd' not in --only".into())
    );
    executor.only = Some(allowed());
    executor.exclude.insert("artd".into());
    assert_eq!(
        request(&mut executor, "aidl/artd"),
        Effect::Skipped("'artd' excluded".into())
    );
    executor.exclude.clear();
    for name in ["artd_pre_reboot", "dexopt_chroot_setup"] {
        assert_eq!(
            request(&mut executor, &format!("aidl/{name}")),
            Effect::Skipped(format!("'{name}' not in --only"))
        );
        assert_eq!(
            executor.start_service(name).unwrap(),
            Effect::Skipped(format!("'{name}' not in --only"))
        );
    }
    assert!(executor.launches.is_empty());
    assert!(
        matches!(request(&mut executor, "aidl/artd"), Effect::Launch { service, .. } if service == "artd")
    );
    let spec = &executor.launches[0].spec;
    assert_eq!(spec.argv, ["/apex/com.android.art/bin/artd"]);
    assert_eq!((spec.identity.uid, spec.identity.gid), (1082, 1082));
    assert!(spec.identity.groups.is_empty());
    assert_eq!(
        spec.identity.capabilities,
        Capabilities::for_service(1082, Some(0xf))
    );
    assert_eq!(
        request(&mut executor, "aidl/artd"),
        Effect::Applied("already running".into())
    );
    assert_eq!(executor.launches.len(), 1);
    drop(executor);
    common::make_writable(&root);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn oneshot_artd_waits_for_next_request_after_idle_exit_or_death() {
    let (mut executor, root) = fixture("artd-restart");
    request(&mut executor, "aidl/artd");
    for exit in [Exit::Code(0), Exit::Signal(libc::SIGSEGV)] {
        let previous = executor.launches.last().unwrap().clone();
        executor.handle_exit(previous.pid, exit);
        assert_eq!(
            executor
                .props
                .borrow()
                .areas()
                .get("init.svc.artd")
                .as_deref(),
            Some("stopped")
        );
        assert!(executor.supervisor.record("artd").unwrap().flags & flags::DISABLED != 0);
        let count = executor.launches.len();
        core_start(&mut executor);
        executor.poll_processes();
        assert_eq!(executor.launches.len(), count);
        assert!(matches!(
            request(&mut executor, "aidl/artd"),
            Effect::Launch { .. }
        ));
        let current = executor.launches.last().unwrap();
        assert_ne!(current.pid, previous.pid);
        assert_eq!(current.spec.generation, previous.spec.generation + 1);
        assert_eq!(current.spec.identity, previous.spec.identity);
        assert_eq!(
            executor
                .props
                .borrow()
                .areas()
                .get("init.svc.artd")
                .as_deref(),
            Some("running")
        );
    }
    drop(executor);
    common::make_writable(&root);
    std::fs::remove_dir_all(root).unwrap();
}
