//! Bounded native regular-I/O cost evidence for original stub extraction (#1263).
use super::*;
use crate::sys::{attrs, fdtab, fs as guest, posix_locks};
use aim_storage::{posix_broker, posix_control, process_namespace::ProcessIdentity};
use std::{ffi::CString, fs, io::{Read, Write}, time::{Duration, Instant}};

const SIZE: usize = 1024 * 1024;
fn open(name: &str, flags: u64) -> i32 {
    let name = CString::new(name).unwrap();
    let result = guest::openat([vfs::LINUX_AT_FDCWD as u64, name.as_ptr() as u64, flags, 0o600, 0, 0]);
    assert!(result >= 0, "guest open: {result}"); result as i32
}
fn close(fd: i32) { assert_eq!(guest::close([fd as u64, 0, 0, 0, 0, 0]), 0); }
fn read_guest(fd: i32, chunk: usize, expected: &[u8]) -> u128 {
    assert_eq!(guest::lseek([fd as u64, 0, 0, 0, 0, 0]), 0);
    let start = Instant::now(); let mut bytes = vec![0; chunk]; let mut collected = Vec::new();
    loop {
        let n = guest::read([fd as u64, bytes.as_mut_ptr() as u64, chunk as u64, 0, 0, 0]);
        assert!(n >= 0, "guest read: {n}"); if n == 0 { break; }
        collected.extend_from_slice(&bytes[..n as usize]);
    }
    let elapsed = start.elapsed().as_nanos(); assert_eq!(collected, expected); elapsed
}
fn read_host(path: &Path, chunk: usize, expected: &[u8]) -> u128 {
    let mut file = File::open(path).unwrap(); let start = Instant::now();
    let mut collected = Vec::new(); let mut bytes = vec![0; chunk];
    loop { let n = file.read(&mut bytes).unwrap(); if n == 0 { break; } collected.extend_from_slice(&bytes[..n]); }
    let elapsed = start.elapsed().as_nanos(); assert_eq!(collected, expected); elapsed
}
fn measure(mut action: impl FnMut()) -> u128 {
    let start = Instant::now(); for _ in 0..SIZE / 512 { action(); } start.elapsed().as_nanos()
}

#[test]
fn real_regular_owner_costs_preserve_bytes_offsets_and_verity_admission() {
    const MARKER: &str = "aim-regular-owner-cost-child";
    if !std::env::args().any(|argument| argument == MARKER) {
        struct Child(std::process::Child);
        impl Drop for Child {
            fn drop(&mut self) {
                if self.0.try_wait().unwrap().is_none() { self.0.kill().unwrap(); }
                self.0.wait().unwrap();
            }
        }
        let mut child = Child(std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "sys::regular_file::owner_cost_tests::real_regular_owner_costs_preserve_bytes_offsets_and_verity_admission", "--nocapture", "--skip", MARKER])
            .spawn().unwrap());
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(status) = child.0.try_wait().unwrap() { assert!(status.success()); return; }
            assert!(Instant::now() < deadline, "owned regular-I/O fixture exceeded20s");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    let (_view, root) = vfs::test_view(); fdtab::install_storage_registrar().unwrap();
    let process = ProcessIdentity::running(std::process::id() as i32).unwrap();
    let controller = posix_broker::Controller::start(posix_broker::Config {
        endpoint: format!("dev.aim.regular-cost.{}", process.host_pid),
        holder: aim_paths::root().join("target/debug/aim-lock-holder"),
        startup_timeout: Duration::from_secs(3),
    }).unwrap();
    let owner = posix_control::Owner { process, guest_pid: 1 };
    controller.register_guest(owner).unwrap();
    let control = posix_control::Client::lookup(&controller.owner_config().unwrap().endpoint).unwrap();
    posix_locks::install(posix_locks::Client::new(control, owner, process).unwrap()).unwrap();
    assert!(attrs::recording());
    let name = "/data/regular-owner-cost-input";
    let path = vfs::lookup(name).0;
    let expected = (0..SIZE).map(|i| ((i * 37 + i / 257) & 255) as u8).collect::<Vec<_>>();
    fs::write(&path, &expected).unwrap();
    let fd = open(name, 0);
    let description = match fdtab::get(fd).unwrap() { fdtab::Kind::Regular(value) => value, _ => panic!("real regular owner missing") };
    let alias = guest::dup([fd as u64, 0, 0, 0, 0, 0]) as i32; assert!(alias >= 0);
    let duplicate = match fdtab::get(alias).unwrap() { fdtab::Kind::Regular(value) => value, _ => panic!("duplicate owner missing") };
    assert!(Arc::ptr_eq(&description, &duplicate));
    let mut first = [0; 32]; assert_eq!(guest::read([fd as u64, first.as_mut_ptr() as u64, 32, 0, 0, 0]), 32);
    assert_eq!(guest::lseek([alias as u64, 0, 1, 0, 0, 0]), 32);
    let mut second = [0; 32]; assert_eq!(guest::read([alias as u64, second.as_mut_ptr() as u64, 32, 0, 0, 0]), 32);
    assert_eq!(&first, &expected[..32]); assert_eq!(&second, &expected[32..64]); close(alias); drop(duplicate);
    let direct512 = read_host(&path, 512, &expected);
    let direct32k = read_host(&path, 32768, &expected);
    let owned512 = read_guest(fd, 512, &expected);
    let owned32k = read_guest(fd, 32768, &expected);
    let pin = fdtab::pin_guest(fd).unwrap();
    let identity = measure(|| assert_eq!(Identity::from_fd(pin.descriptor()).unwrap(), description.identity));
    let offset = measure(|| { let _lock = description.inode.offset_lock().unwrap(); });
    let admission_ns = measure(|| { let _admission = description.store.lock_inode(&pin.descriptor()).unwrap(); });
    let retained_admission = measure(|| { let _admission = description.admission.lock().unwrap(); });
    let metadata = measure(|| { assert!(description.store.lookup(description.identity).unwrap().is_none()); });
    drop(pin); close(fd); drop(description);

    let output_name = "/data/regular-owner-cost-output"; let output = vfs::lookup(output_name).0;
    let fd = open(output_name, 0o102); // O_CREAT | O_RDWR, Linux values.
    let description = match fdtab::get(fd).unwrap() { fdtab::Kind::Regular(value) => value, _ => panic!("writer owner missing") };
    let pin = fdtab::pin_guest(fd).unwrap();
    assert!(description.writer.is_some());
    let readonly = File::open(&output).unwrap();
    let begin = description.store.lock_inode(&readonly).unwrap();
    assert!(matches!(begin.begin_enable(), Err(aim_storage::fsverity::Error::Linux(26)))); drop(begin); drop(readonly);
    let write_check = measure(|| description.check_write().unwrap());
    let start = Instant::now();
    for bytes in expected.chunks(8192) {
        assert_eq!(guest::write([fd as u64, bytes.as_ptr() as u64, bytes.len() as u64, 0, 0, 0]), bytes.len() as i64);
    }
    let write8k = start.elapsed().as_nanos();
    assert_eq!(guest::lseek([fd as u64, 0, 1, 0, 0, 0]), SIZE as i64);
    assert_eq!(guest::pwrite64([fd as u64, expected.as_ptr() as u64, 16, 0, 0, 0]), 16);
    assert_eq!(guest::lseek([fd as u64, 0, 1, 0, 0, 0]), SIZE as i64);
    drop(pin); close(fd); drop(description); assert_eq!(fs::read(&output).unwrap(), expected);
    let direct_output = root.join("data/regular-owner-cost-host-output");
    let mut native = File::create(&direct_output).unwrap();
    let start = Instant::now(); for bytes in expected.chunks(8192) { native.write_all(bytes).unwrap(); }
    let direct_write8k = start.elapsed().as_nanos(); drop(native);
    assert_eq!(fs::read(&direct_output).unwrap(), expected);

    let fd = open(output_name, 0); // Open before ENABLE; retained admission must see new proof.
    let source = File::open(&output).unwrap();
    let runtime = vfs::runtime_dir().unwrap();
    let config = configuration().unwrap().unwrap();
    let admission = config.store.lock_inode(&source).unwrap(); let enable = admission.begin_enable().unwrap(); drop(admission);
    let prepared = enable.build(aim_storage::fsverity::BuildOptions::new(1, 4096, vec![], 16384, 4096).unwrap(), &[], || false).unwrap(); enable.commit(prepared).unwrap();
    assert_eq!(guest::openat([vfs::LINUX_AT_FDCWD as u64, CString::new(output_name).unwrap().as_ptr() as u64, 1, 0, 0, 0]), -(crate::errno::EPERM as i64));
    let mut verified = [0; 4096];
    assert_eq!(guest::read([fd as u64, verified.as_mut_ptr() as u64, 4096, 0, 0, 0]), 4096);
    assert_eq!(&verified, &expected[..4096]);
    use std::os::unix::fs::FileExt;
    let fault = fs::OpenOptions::new().write(true).open(&output).unwrap();
    fault.write_all_at(&[expected[0] ^ 255], 0).unwrap(); drop(fault);
    assert_eq!(guest::lseek([fd as u64, 0, 0, 0, 0, 0]), 0);
    assert_eq!(guest::read([fd as u64, verified.as_mut_ptr() as u64, 4096, 0, 0, 0]), -(crate::errno::EIO as i64));
    let fault = fs::OpenOptions::new().write(true).open(&output).unwrap();
    fault.write_all_at(&expected[..1], 0).unwrap(); drop(fault);
    let proof_metadata = config.store.lookup(Identity::from_fd(source.as_fd()).unwrap()).unwrap().unwrap();
    use std::os::fd::{AsFd, AsRawFd};
    let proof = PathBuf::from(crate::xrt::fd_path(proof_metadata.backing_descriptor().as_raw_fd()).unwrap());
    assert!(fs::canonicalize(&proof).unwrap().starts_with(fs::canonicalize(root).unwrap())); drop(proof_metadata);
    let fault = fs::OpenOptions::new().write(true).open(proof).unwrap();
    fault.write_all_at(&[0], 0).unwrap(); drop(fault);
    assert_eq!(guest::read([fd as u64, verified.as_mut_ptr() as u64, 4096, 0, 0, 0]), -(crate::errno::EIO as i64));
    close(fd); drop(source);
    println!("REGULAR_OWNER_COST bytes={SIZE} iterations={} direct512_ns={direct512} direct32k_ns={direct32k} owned512_ns={owned512} owned32k_ns={owned32k} identity_ns={identity} offset_ns={offset} admission_ns={admission_ns} retained_admission_ns={retained_admission} metadata_ns={metadata} write_check_ns={write_check} write8k_ns={write8k} direct_write8k_ns={direct_write8k} runtime={}", SIZE/512, runtime.display());
    controller.shutdown().unwrap();
    for path in [path, output, direct_output] { fs::remove_file(path).unwrap(); }
    fs::remove_dir_all(root).unwrap();
}
