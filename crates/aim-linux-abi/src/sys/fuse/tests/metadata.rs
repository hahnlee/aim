use super::*;
use crate::{sys::fuse_client, vfs::FuseRoute};

fn route(connection: &Connection, relative: &str) -> FuseRoute {
    FuseRoute {
        session: connection.key.transport().to_owned(),
        relative: relative.into(),
        uid: 0,
        gid: 0,
        allow_other: true,
        default_permissions: false,
        read_only: false,
    }
}

#[test]
fn mkdir_existing_fuse_dentries_match_linux_namei() {
    let (_guard, root) = crate::vfs::test_view();
    let connection = Connection::new();
    initialize(&connection);
    let lower = root.join("mkdir-lower");
    fs::create_dir_all(lower.join("0/existing")).unwrap();
    fs::write(lower.join("0/file"), b"").unwrap();
    std::os::unix::fs::symlink("absent", lower.join("0/link")).unwrap();
    let mount = "/mnt/fuse-mkdir";
    let options =
        crate::sys::fuse_mount::parse(b"fd=17,rootmode=40000,user_id=0,group_id=0,allow_other")
            .unwrap();
    crate::vfs::add_fuse_mount(
        mount,
        connection.key.transport().to_owned(),
        lower.clone(),
        "fuse",
        &options,
        false,
    )
    .unwrap();
    let device = connection.fd;
    let server = std::thread::spawn(move || {
        let daemon = aim_emulated_storage::Daemon::new(lower);
        let mut mkdirs = 0;
        loop {
            let packet = next(device);
            let op = u32::from_le_bytes(packet[4..8].try_into().unwrap());
            if op == 38 {
                respond(device, &packet, 0, &[]);
                return mkdirs;
            }
            if op == 9 {
                mkdirs += 1;
            }
            if let Some(reply) = daemon.handle(&packet) {
                write_device(device, &reply).unwrap();
            }
        }
    });
    // LTP mkdir/mkdirat EEXIST cases: directory, regular file, symlink,
    // trailing slash and mount root; the existing dentry never reaches MKDIR.
    for suffix in [
        "",
        "/",
        "/.",
        "/0/existing",
        "/0/existing/",
        "/0/file",
        "/0/link",
    ] {
        let path = std::ffi::CString::new(format!("{mount}{suffix}")).unwrap();
        assert_eq!(
            crate::sys::fsops::mkdirat([
                crate::vfs::LINUX_AT_FDCWD as u64,
                path.as_ptr() as u64,
                0o755,
                0,
                0,
                0
            ]),
            -(crate::errno::EEXIST as i64),
            "{suffix}"
        );
    }
    for (suffix, expected) in [
        ("/0/new", 0),
        ("/absent/child", -(crate::errno::ENOENT as i64)),
        ("/0/file/child", -(crate::errno::ENOTDIR as i64)),
    ] {
        let path = std::ffi::CString::new(format!("{mount}{suffix}")).unwrap();
        assert_eq!(
            crate::sys::fsops::mkdirat([
                crate::vfs::LINUX_AT_FDCWD as u64,
                path.as_ptr() as u64,
                0o755,
                0,
                0,
                0
            ]),
            expected,
            "{suffix}"
        );
    }
    Client::from_key(&connection.key)
        .unwrap()
        .request(38, 1, &[], 0, 0, 1)
        .unwrap();
    assert_eq!(server.join().unwrap(), 1);
    assert!(crate::vfs::remove_mount(mount).unwrap());
}

fn entry(node: u64, entry_nanos: u32, attr_nanos: u32) -> Vec<u8> {
    let mut bytes = vec![0; 128];
    bytes[..8].copy_from_slice(&node.to_le_bytes());
    bytes[8..16].copy_from_slice(&7u64.to_le_bytes());
    bytes[32..36].copy_from_slice(&entry_nanos.to_le_bytes());
    bytes[36..40].copy_from_slice(&attr_nanos.to_le_bytes());
    bytes[40..48].copy_from_slice(&node.to_le_bytes());
    bytes[100..104].copy_from_slice(&0o040755u32.to_le_bytes());
    bytes
}

#[test]
fn notification_during_lookup_does_not_cache_the_older_reply() {
    let connection = Connection::new();
    initialize(&connection);
    for invalidation in [3u32, 6] {
        let client = Client::from_key(&connection.key).unwrap();
        let pending =
            std::thread::spawn(move || client.request(1, 1, b"child\0", 0, 0, 1).unwrap());
        let packet = next(connection.fd);
        assert_eq!(u32::from_le_bytes(packet[4..8].try_into().unwrap()), 1);
        let mut body = 1u64.to_le_bytes().to_vec();
        if invalidation == 6 {
            body.extend(2u64.to_le_bytes());
        }
        body.extend(5u32.to_le_bytes());
        body.extend(0u32.to_le_bytes());
        body.extend(b"child");
        let mut notify = ((16 + body.len()) as u32).to_le_bytes().to_vec();
        notify.extend(invalidation.to_le_bytes());
        notify.extend(0u64.to_le_bytes());
        notify.extend(body);
        write_device(connection.fd, &notify).unwrap();
        respond(
            connection.fd,
            &packet,
            0,
            &entry(2, 900_000_000, 900_000_000),
        );
        pending.join().unwrap();
        let client = Client::from_key(&connection.key).unwrap();
        let release = std::thread::spawn(move || {
            client
                .request_no_reply(FORGET, 2, &1u64.to_le_bytes(), 0, 0, 1)
                .unwrap()
        });
        let packet = next(connection.fd);
        assert_eq!(u32::from_le_bytes(packet[4..8].try_into().unwrap()), FORGET);
        release.join().unwrap();
    }
}

#[test]
fn fuse_zero_negative_timeouts_mutations_and_retained_users() {
    let connection = Connection::new();
    initialize(&connection);
    let device = connection.fd;
    let server = std::thread::spawn(move || {
        let (mut lookups, mut attrs, mut refs) = (0, 0, 0i64);
        loop {
            let packet = next(device);
            match u32::from_le_bytes(packet[4..8].try_into().unwrap()) {
                1 => {
                    lookups += 1;
                    let (node, timeout) = match &packet[40..] {
                        b"zero\0" => (2, 0),
                        b"denied\0" => (3, 0),
                        b"missing\0" => (0, 900_000_000),
                        _ => (2, 900_000_000),
                    };
                    if node != 0 {
                        refs += 1;
                    }
                    respond(device, &packet, 0, &entry(node, timeout, timeout));
                }
                3 => {
                    attrs += 1;
                    if u64::from_le_bytes(packet[16..24].try_into().unwrap()) == 3 {
                        respond(device, &packet, -crate::errno::EACCES, &[]);
                        continue;
                    }
                    let mut bytes = vec![0; 16];
                    bytes.extend(&entry(2, 0, 0)[40..]);
                    respond(device, &packet, 0, &bytes);
                }
                10 => respond(device, &packet, 0, &[]),
                9 => {
                    refs += 1;
                    respond(device, &packet, 0, &entry(2, 900_000_000, 900_000_000));
                }
                FORGET => {
                    refs -= u64::from_le_bytes(packet[40..48].try_into().unwrap()) as i64;
                    assert!(refs >= 0);
                }
                38 => {
                    respond(device, &packet, 0, &[]);
                    return (lookups, attrs, refs);
                }
                op => panic!("unexpected opcode {op}"),
            }
        }
    });
    let zero = route(&connection, "zero");
    for _ in 0..2 {
        fuse_client::stat(&zero, None, None).unwrap();
    }
    let missing = route(&connection, "missing");
    assert_eq!(
        fuse_client::stat(&route(&connection, "denied"), None, None).err(),
        Some(crate::errno::EACCES)
    );
    for _ in 0..2 {
        assert_eq!(fuse_client::lookup(&missing), Err(crate::errno::ENOENT));
    }
    let child = route(&connection, "child");
    // Two clients share one real lookup; keep both virtual references while
    // an unlink invalidates the entry. Neither may trigger an early FORGET.
    assert_eq!(fuse_client::lookup(&child), Ok(2));
    assert_eq!(fuse_client::lookup(&child), Ok(2));
    fuse_client::unlink(&child, false).unwrap();
    fuse_client::forget(&child, 2, 1).unwrap();
    let client = Client::from_key(&connection.key).unwrap();
    // BATCH_FORGET releases the other cached user too.
    let mut batch = 1u32.to_le_bytes().to_vec();
    batch.extend(0u32.to_le_bytes());
    batch.extend(2u64.to_le_bytes());
    batch.extend(1u64.to_le_bytes());
    client
        .request_no_reply(BATCH_FORGET, 0, &batch, 0, 0, 1)
        .unwrap();
    // Mutation also invalidated the negative dentry.
    assert_eq!(fuse_client::lookup(&missing), Err(crate::errno::ENOENT));
    // Creation supplies the dentry/attributes without another daemon lookup.
    let mut mkdir = vec![0; 8];
    mkdir.extend(b"new\0");
    client.request(9, 1, &mkdir, 0, 0, 1).unwrap();
    let created = route(&connection, "new");
    fuse_client::forget(&created, 2, 1).unwrap();
    fuse_client::stat(&created, None, None).unwrap();
    fuse_client::unlink(&created, false).unwrap();
    client.request(38, 1, &[], 0, 0, 1).unwrap();
    assert_eq!(server.join().unwrap(), (6, 3, 0));
}

#[test]
fn fuse_reply_timeouts_notifications_and_lookup_references() {
    let connection = Connection::new();
    initialize(&connection);
    let device = connection.fd;
    let server = std::thread::spawn(move || {
        let mut lookups = 0;
        let mut getattrs = 0;
        let mut refs = 0i64;
        loop {
            let packet = next(device);
            let op = u32::from_le_bytes(packet[4..8].try_into().unwrap());
            match op {
                1 => {
                    lookups += 1;
                    refs += 1;
                    respond(device, &packet, 0, &entry(2, 300_000_000, 100_000_000));
                }
                3 => {
                    getattrs += 1;
                    let mut attr = vec![0; 16];
                    // GETATTR's own timeout also has to be honored.
                    attr[8..12].copy_from_slice(&100_000_000u32.to_le_bytes());
                    attr.extend(&entry(2, 0, 0)[40..]);
                    respond(device, &packet, 0, &attr);
                }
                FORGET => {
                    refs -= u64::from_le_bytes(packet[40..48].try_into().unwrap()) as i64;
                    assert!(refs >= 0);
                }
                17 => respond(device, &packet, 0, &[0; 80]),
                38 => {
                    respond(device, &packet, 0, &[]);
                    return (lookups, getattrs, refs);
                }
                _ => panic!("unexpected opcode {op}"),
            }
        }
    });
    let path = route(&connection, "child");
    for _ in 0..3 {
        fuse_client::stat(&path, None, None).unwrap();
    }
    std::thread::sleep(Duration::from_millis(130));
    // entry remains valid, but the shorter attr timeout requires GETATTR.
    for _ in 0..2 {
        fuse_client::stat(&path, None, None).unwrap();
    }
    let mut notify = vec![0; 40];
    notify[..4].copy_from_slice(&40u32.to_le_bytes());
    notify[4..8].copy_from_slice(&2i32.to_le_bytes());
    notify[16..24].copy_from_slice(&2u64.to_le_bytes());
    notify[24..32].copy_from_slice(&(-1i64).to_le_bytes());
    write_device(device, &notify).unwrap();
    fuse_client::stat(&path, None, None).unwrap();
    // Expiring entries release all real LOOKUPs only after virtual users go.
    std::thread::sleep(Duration::from_millis(330));
    fuse_client::statfs(&path).unwrap();
    Client::from_key(&connection.key)
        .unwrap()
        .request(38, 1, &[], 0, 0, 1)
        .unwrap();
    assert_eq!(server.join().unwrap(), (1, 2, 0));
}

mod benchmark;

#[test]
fn fuse_namei_walk_is_linear_and_rechecks_zero_timeout_entries() {
    use std::sync::{Arc, Mutex};
    let (_guard, root) = crate::vfs::test_view();
    let connection = Connection::new();
    let lower = root.join("linear-fuse-lower");
    fs::create_dir_all(lower.join("0/a/b")).unwrap();
    fs::write(lower.join("0/a/file"), b"").unwrap();
    let mount = "/mnt/linear-fuse";
    std::os::unix::fs::symlink("b", lower.join("0/a/rel")).unwrap();
    std::os::unix::fs::symlink(format!("{mount}/0/a/b"), lower.join("0/abs")).unwrap();
    let options =
        crate::sys::fuse_mount::parse(b"fd=17,rootmode=40000,user_id=0,group_id=0,allow_other")
            .unwrap();
    crate::vfs::add_fuse_mount(
        mount,
        connection.key.transport().to_owned(),
        lower.clone(),
        "fuse",
        &options,
        false,
    )
    .unwrap();
    let counts = Arc::new(Mutex::new(std::collections::BTreeMap::<u32, usize>::new()));
    let recorded = counts.clone();
    let device = connection.fd;
    let daemon_lower = lower.clone();
    // An unmounted device returns EPERM even for a blocking read.
    let client = Client::new(&connection.key);
    client.mount().unwrap();
    let daemon = std::thread::spawn(move || {
        let daemon = aim_emulated_storage::Daemon::new(daemon_lower);
        loop {
            let packet = next(device);
            let op = u32::from_le_bytes(packet[4..8].try_into().unwrap());
            *recorded.lock().unwrap().entry(op).or_default() += 1;
            if let Some(reply) = daemon.handle(&packet) {
                write_device(device, &reply).unwrap();
            }
            if op == 38 {
                break;
            }
        }
    });
    let resolve = |suffix: &str| {
        crate::vfs::resolve(
            crate::vfs::LINUX_AT_FDCWD,
            format!("{mount}/{suffix}").as_bytes(),
            true,
        )
    };
    // A STATFS barrier drains FORGETs before each count snapshot. Its own
    // reply remains per-call, including after a failed path walk.
    for expected in [3, 6] {
        assert!(resolve("0/a/b").is_ok());
        client.request(17, 1, &[], 0, 0, 1).unwrap();
        let counts = counts.lock().unwrap();
        assert_eq!(counts[&1], expected);
        assert_eq!(counts[&FORGET], expected);
        assert_eq!(counts[&3], expected / 3); // only the mount root needs GETATTR
    }
    for suffix in ["0/a/rel", "0/abs", "0/a/b/../b", "0/a/missing"] {
        assert!(resolve(suffix).is_ok(), "{suffix}");
    }
    assert_eq!(
        resolve("0/a/missing/child").err(),
        Some(crate::errno::ENOENT)
    );
    assert_eq!(resolve("0/a/file/child").err(), Some(crate::errno::ENOTDIR));
    // default_permissions must still check the current parent mode; an
    // incremental walk must not turn LOOKUP's type into an access grant.
    let permission_mount = "/mnt/linear-fuse-permissions";
    let permission_options = crate::sys::fuse_mount::parse(
        b"fd=17,rootmode=40000,user_id=0,group_id=0,allow_other,default_permissions",
    )
    .unwrap();
    crate::vfs::add_fuse_mount(
        permission_mount,
        connection.key.transport().to_owned(),
        lower.clone(),
        "fuse",
        &permission_options,
        false,
    )
    .unwrap();
    struct Identity(crate::sys::cred::Identity);
    impl Drop for Identity {
        fn drop(&mut self) {
            crate::sys::cred::init(self.0.clone(), None);
        }
    }
    let saved = Identity(crate::sys::cred::current());
    let mut app = saved.0.clone();
    app.uid = [10001; 4];
    app.gid = [10001; 4];
    app.groups.clear();
    crate::sys::cred::init(app, None);
    let path = format!("{permission_mount}/0/a/b");
    assert!(crate::vfs::resolve(crate::vfs::LINUX_AT_FDCWD, path.as_bytes(), true).is_ok());
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(lower.join("0/a"), fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(
        crate::vfs::resolve(crate::vfs::LINUX_AT_FDCWD, path.as_bytes(), true).err(),
        Some(crate::errno::EACCES)
    );
    drop(saved);
    assert!(crate::vfs::remove_mount(permission_mount).unwrap());
    client.request(38, 1, &[], 0, 0, 1).unwrap();
    daemon.join().unwrap();
    assert!(crate::vfs::remove_mount(mount).unwrap());
}

#[test]
fn broker_workers_keep_blocked_requests_and_errors_independent() {
    let connection = Connection::new();
    initialize(&connection);
    let device = connection.fd;
    let daemon = std::thread::spawn(move || {
        for _ in 0..160 {
            let packet = next(device);
            let node = u64::from_le_bytes(packet[16..24].try_into().unwrap());
            if node % 7 == 0 {
                respond(device, &packet, -crate::errno::EACCES, &[]);
            } else {
                let mut attr = vec![0; 104];
                attr[16..24].copy_from_slice(&node.to_le_bytes());
                respond(device, &packet, 0, &attr);
            }
        }
    });
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
    let clients: Vec<_> = (0..8)
        .map(|worker| {
            let client = Client::new(&connection.key);
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                for iteration in 0..20 {
                    let node = worker * 100 + iteration + 2;
                    let result = client.request(3, node, &[0; 16], 0, 0, 1);
                    if node % 7 == 0 {
                        assert_eq!(result.err(), Some(crate::errno::EACCES));
                    } else {
                        assert_eq!(
                            u64::from_le_bytes(result.unwrap().body[16..24].try_into().unwrap()),
                            node
                        );
                    }
                }
            })
        })
        .collect();
    for client in clients {
        client.join().unwrap();
    }
    daemon.join().unwrap();
}

#[test]
fn inherited_and_adopted_fuse_io_keeps_its_owner() {
    if crate::sys::fdtab::isolated_kernel_test(
        "sys::fuse::tests::metadata::inherited_and_adopted_fuse_io_keeps_its_owner",
    ) {
        return;
    }
    use crate::sys::{fdtab, fs as calls};
    use std::os::fd::AsRawFd;
    let (_guard, root) = crate::vfs::test_view();
    let connection = Connection::new();
    initialize(&connection);
    let lower = root.join("inherited-fuse");
    fs::create_dir(&lower).unwrap();
    fs::write(lower.join("file"), b"abcdef").unwrap();
    let device = connection.fd;
    let server = std::thread::spawn(move || {
        let daemon = aim_emulated_storage::Daemon::new(lower);
        let mut reads = 0;
        let mut writes = 0;
        loop {
            let packet = next(device);
            let op = u32::from_le_bytes(packet[4..8].try_into().unwrap());
            if op == 38 {
                respond(device, &packet, 0, &[]);
                return (reads, writes);
            }
            reads += usize::from(op == 15);
            writes += usize::from(op == 16);
            if let Some(reply) = daemon.handle(&packet) {
                write_device(device, &reply).unwrap();
            }
        }
    });
    let fd = fuse_client::open_fd(
        route(&connection, "file"),
        "/mnt/inherited/file".into(),
        2,
        0,
    )
    .unwrap();
    // A raw duplicate has the broker's marker but no process-local Open, as
    // with an inherited or transferred description before adoption.
    let inherited = unsafe { libc::dup(fd) };
    assert!(inherited >= 0);
    fdtab::publish_guest(inherited).unwrap();
    assert!(fuse_client::get(inherited).is_none());
    assert_eq!(marker(inherited), Some(FILE_MARKER));
    let pin = fdtab::pin_guest_io(inherited).unwrap();
    assert!(pin.regular_access().is_none());
    assert!(!pin.socket_allowed());
    let private = pin.descriptor().as_raw_fd();
    assert_ne!(private, inherited);
    assert!(fuse_client::get(private).is_some());
    drop(pin);
    assert!(fuse_client::get(private).is_none());
    let mut bytes = [0u8; 3];
    assert_eq!(
        calls::read([inherited as u64, bytes.as_mut_ptr() as u64, 3, 0, 0, 0]),
        3
    );
    assert_eq!(bytes, *b"abc");
    fdtab::adopt(inherited);
    assert!(fuse_client::get(inherited).is_some());
    assert!(
        fdtab::pin_guest_io(inherited)
            .unwrap()
            .regular_access()
            .is_none()
    );
    assert_eq!(calls::lseek([inherited as u64, 1, 0, 0, 0, 0]), 1);
    assert_eq!(
        calls::write([inherited as u64, b"XY".as_ptr() as u64, 2, 0, 0, 0]),
        2
    );
    assert_eq!(calls::lseek([fd as u64, 0, 0, 0, 0, 0]), 0);
    assert_eq!(
        calls::read([fd as u64, bytes.as_mut_ptr() as u64, 3, 0, 0, 0]),
        3
    );
    assert_eq!(bytes, *b"aXY");
    for fd in [fd, inherited] {
        assert_eq!(calls::close([fd as u64, 0, 0, 0, 0, 0]), 0);
    }
    Client::from_key(&connection.key)
        .unwrap()
        .request(38, 1, &[], 0, 0, 1)
        .unwrap();
    let (reads, writes) = server.join().unwrap();
    assert!(reads > 0);
    assert!(writes > 0);
}
