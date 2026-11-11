//! `mount_all` of a `/data` with `fileencryption=` across boots of one
//! data directory (ADR 0016), with synthetic processes only.

mod common;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Instant;

use aim_android_init::engine::Command;
use aim_android_init::props::build_trie;
use aim_android_init::rc::IdResolver;
use aim_guest_init::executor::GuestExecutor;
use aim_guest_init::fsops::FsOps;
use aim_guest_init::launch::{DryRunLauncher, LinuxRun, LinuxRunOptions};
use aim_guest_init::mount::DataMount;
use aim_guest_init::paths::Layout;
use aim_guest_init::props::heap_properties;
use aim_guest_init::supervisor::Planner;

const FSTAB: &str =
    "/dev/block/vdb /data ext4 noatime latemount,wait,fileencryption=aes-256-xts:aes-256-cts:v2\n";

/// One boot's executor of the data directory in `root`, its `/data`
/// mounted as `mount_all` mounts it.
fn boot(root: &Path, image: &Path) -> GuestExecutor {
    let layout = Layout::new(
        image.to_path_buf(),
        root.join("data"),
        Some(root.join("run")),
    );
    layout.prepare().unwrap();
    let info = build_trie(&[], "u:object_r:default_prop:s0", "string").unwrap();
    let props = Rc::new(RefCell::new(heap_properties(info).unwrap()));
    let launcher = DryRunLauncher::new(LinuxRun {
        binary: "linux-run".into(),
        image: image.to_path_buf(),
        path_map_file: layout.path_map_file(),
        binder: None,
        gpu: None,
        vulkan: None,
        display: None,
        trace: false,
        options: LinuxRunOptions::CONTRACT,
    });
    let planner = Planner {
        map: layout.path_map(),
        layout: layout.clone(),
        ids: IdResolver::builtin(),
        vendor_api_level: 36,
        env: vec![],
        rlimits: vec![],
        boot_epoch: Instant::now(),
    };
    let fs = FsOps::new(layout.path_map(), layout.fs_attrs_file(), false);
    let mut executor = GuestExecutor::new(props, Box::new(launcher), planner, fs, None);
    let data = DataMount::start(layout, None, None, None, Instant::now()).unwrap();
    executor.data = Some(Rc::new(RefCell::new(data)));
    executor
}

fn mount_all(executor: &mut GuestExecutor) -> Result<(), String> {
    executor
        .run_command(&Command::MountAll {
            args: vec!["/vendor/etc/fstab.aim".into(), "--late".into()],
        })
        .map(drop)
}

fn image(root: &Path) -> PathBuf {
    let image = common::fixture_image(root);
    std::fs::create_dir_all(image.join("vendor/etc")).unwrap();
    std::fs::write(image.join("vendor/etc/fstab.aim"), FSTAB).unwrap();
    image
}

/// A boot that ends after its first encrypted directory was made, before
/// any policy was recorded, leaves a /data the next boot accepts.
#[test]
fn a_boot_ended_before_the_first_policy_is_not_taken_for_one_from_before_fbe() {
    let root = common::temp_dir("fbe-mount-early-end");
    let image = image(&root);
    let mut first = boot(&root, &image);
    mount_all(&mut first).unwrap();
    assert!(first.fatal.is_none(), "{:?}", first.fatal);
    // `mkdir /data/bootchart ... encryption=Require` made the directory;
    // the boot ended before its policy was set.
    std::fs::create_dir(root.join("data/data/bootchart")).unwrap();
    drop(first);
    let mut second = boot(&root, &image);
    mount_all(&mut second).unwrap();
    assert!(second.fatal.is_none(), "{:?}", second.fatal);
    drop(second);
}

/// A data directory the original image's LockSettings protects is refused
/// to an app host image, naming both.
#[test]
fn an_app_host_image_refuses_a_data_directory_the_original_protects() {
    let root = common::temp_dir("fbe-mount-protector");
    let image = image(&root);
    let mut first = boot(&root, &image);
    mount_all(&mut first).unwrap();
    assert!(first.fatal.is_none(), "{:?}", first.fatal);
    drop(first);
    let services = image.join("system/etc/aim/app-host-services");
    std::fs::create_dir_all(services.parent().unwrap()).unwrap();
    std::fs::write(&services, "vold\n").unwrap();
    let mut second = boot(&root, &image);
    assert!(mount_all(&mut second).is_err());
    let fatal = second.fatal.clone().unwrap();
    assert!(
        fatal.contains("protected by the original LockSettingsService")
            && fatal.contains("this image's is the app host"),
        "{fatal}"
    );
    drop(second);
}

/// `mount_all` gives what an earlier build wrote below an encrypted
/// directory without a policy its directory's; one it cannot repair ends
/// the boot, naming the inode and its errno.
#[test]
fn mount_all_repairs_policy_less_inodes_and_names_one_it_cannot() {
    let root = common::temp_dir("fbe-mount-repair");
    let image = image(&root);
    let mut first = boot(&root, &image);
    mount_all(&mut first).unwrap();
    drop(first);
    let system = root.join("data/data/system");
    std::fs::create_dir(&system).unwrap();
    let policy = [&[2u8, 1, 4, 2, 0, 0, 0, 0][..], &[7; 16]].concat();
    let owner = aim_storage::fscrypt::Owner::locate(&root.join("run"))
        .unwrap()
        .unwrap();
    owner.set_policy(&system, &policy).unwrap();
    // What an earlier build left: no mark, a file without a policy, in a
    // directory this boot cannot read.
    std::fs::remove_file(root.join("data/fscrypt/whole")).unwrap();
    let locked = system.join("locked");
    std::fs::create_dir(&locked).unwrap();
    aim_storage::fscrypt::inherit(&system, &locked).unwrap();
    std::fs::write(locked.join("settings.xml"), "").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0)).unwrap();
    let mut second = boot(&root, &image);
    assert!(mount_all(&mut second).is_err());
    let fatal = second.fatal.clone().unwrap();
    assert!(
        fatal.contains(&locked.display().to_string()) && fatal.contains("os error 13"),
        "{fatal}"
    );
    drop(second);
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut third = boot(&root, &image);
    mount_all(&mut third).unwrap();
    assert!(third.fatal.is_none(), "{:?}", third.fatal);
    let placed = aim_storage::fscrypt::context(&locked.join("settings.xml")).unwrap();
    assert_eq!(placed.unwrap()[..24], policy[..]);
    assert!(
        aim_storage::fscrypt::policy_less(&root.join("data/data"))
            .unwrap()
            .is_empty()
    );
    drop(third);
}
