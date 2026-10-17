use super::*;

/// A request of `opcode` to `node` from `uid`, with `body`.
fn request(opcode: u32, node: u64, uid: u32, body: &[u8]) -> Vec<u8> {
    let mut m = Vec::new();
    m.extend(((40 + body.len()) as u32).to_le_bytes());
    m.extend(opcode.to_le_bytes());
    m.extend(7u64.to_le_bytes());
    m.extend(node.to_le_bytes());
    m.extend(uid.to_le_bytes());
    m.extend(0u32.to_le_bytes());
    m.extend(1u32.to_le_bytes());
    m.extend([0; 4]);
    m.extend(body);
    m
}

/// The reply's error and payload.
fn call(d: &Daemon, opcode: u32, node: u64, uid: u32, body: &[u8]) -> (i32, Vec<u8>) {
    let reply = d.handle(&request(opcode, node, uid, body)).unwrap();
    assert_eq!(u32_at(&reply, 0).unwrap() as usize, reply.len());
    assert_eq!(u64_at(&reply, 8), Some(7));
    (i32::from_le_bytes(reply[4..8].try_into().unwrap()), reply[16..].to_vec())
}

fn cstr(s: &str) -> Vec<u8> {
    let mut v = s.as_bytes().to_vec();
    v.push(0);
    v
}

fn lower() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("aim-emulated-storage-{}-{:?}", std::process::id(), std::thread::current().id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("0/Download")).unwrap();
    fs::create_dir_all(dir.join("10")).unwrap();
    dir
}

const APP: u32 = 10_213;

#[test]
fn init_agrees_to_the_protocol() {
    let d = Daemon::new(lower());
    let mut body = Vec::new();
    for v in [7u32, 39, 128 * 1024, 1 | 32 | 4] {
        body.extend(v.to_le_bytes());
    }
    let (error, out) = call(&d, op::INIT, 0, 0, &body);
    assert_eq!((error, out.len()), (0, 64));
    assert_eq!((u32_at(&out, 0), u32_at(&out, 4), u32_at(&out, 12), u32_at(&out, 20)), (Some(7), Some(39), Some(1 | 32), Some(MAX_WRITE)));
}

#[test]
fn an_app_writes_and_reads_a_file_of_its_user() {
    let dir = lower();
    let d = Daemon::new(&dir);
    let (error, user) = call(&d, op::LOOKUP, 1, APP, &cstr("0"));
    assert_eq!(error, 0);
    let user = u64_at(&user, 0).unwrap();
    let (_, download) = call(&d, op::LOOKUP, user, APP, &cstr("Download"));
    let download = u64_at(&download, 0).unwrap();
    // CREATE O_RDWR 0666 with umask 022.
    let mut body = Vec::new();
    for v in [O_RDWR, 0o100666, 0o022, 0] {
        body.extend(v.to_le_bytes());
    }
    body.extend(cstr("a.txt"));
    let (error, created) = call(&d, op::CREATE, download, APP, &body);
    assert_eq!(error, 0);
    assert_eq!(u32_at(&created, 40 + 60).unwrap() & 0o777, 0o644);
    let fh = u64_at(&created, 128).unwrap();
    let mut write = Vec::new();
    write.extend(fh.to_le_bytes());
    write.extend(0u64.to_le_bytes());
    write.extend(5u32.to_le_bytes());
    write.extend([0; 20]);
    write.extend(b"hello");
    let (error, written) = call(&d, op::WRITE, 0, APP, &write);
    assert_eq!((error, u32_at(&written, 0)), (0, Some(5)));
    let mut read = Vec::new();
    read.extend(fh.to_le_bytes());
    read.extend(1u64.to_le_bytes());
    read.extend(100u32.to_le_bytes());
    read.extend([0; 20]);
    assert_eq!(call(&d, op::READ, 0, APP, &read), (0, b"ello".to_vec()));
    assert_eq!(fs::read(dir.join("0/Download/a.txt")).unwrap(), b"hello");
    let mut release = fh.to_le_bytes().to_vec();
    release.extend([0; 16]);
    assert_eq!(call(&d, op::RELEASE, 0, APP, &release).0, 0);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn an_app_reaches_only_its_own_user() {
    let dir = lower();
    let d = Daemon::new(&dir);
    assert_eq!(call(&d, op::LOOKUP, 1, APP, &cstr("10")).0, -errno::ENOENT);
    assert_eq!(call(&d, op::LOOKUP, 1, 1000, &cstr("10")).0, 0);
    // The root lists only the caller's user.
    let (_, open) = call(&d, op::OPENDIR, 1, APP, &[0; 8]);
    let mut read = u64_at(&open, 0).unwrap().to_le_bytes().to_vec();
    read.extend(0u64.to_le_bytes());
    read.extend(4096u32.to_le_bytes());
    read.extend([0; 20]);
    let (_, listing) = call(&d, op::READDIR, 0, APP, &read);
    let mut names = Vec::new();
    let mut at = 0;
    while at < listing.len() {
        let len = u32_at(&listing, at + 16).unwrap() as usize;
        names.push(String::from_utf8(listing[at + 24..at + 24 + len].to_vec()).unwrap());
        at += (24 + len).next_multiple_of(8);
    }
    assert_eq!(names, [".", "..", "0"]);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn rename_moves_the_nodes_below() {
    let dir = lower();
    let d = Daemon::new(&dir);
    let user = u64_at(&call(&d, op::LOOKUP, 1, APP, &cstr("0")).1, 0).unwrap();
    let mut mkdir = Vec::new();
    mkdir.extend(0o775u32.to_le_bytes());
    mkdir.extend(0u32.to_le_bytes());
    mkdir.extend(cstr("a"));
    let a = u64_at(&call(&d, op::MKDIR, user, APP, &mkdir).1, 0).unwrap();
    let mut mknod = Vec::new();
    for v in [0o100644u32, 0, 0, 0] {
        mknod.extend(v.to_le_bytes());
    }
    mknod.extend(cstr("f"));
    let f = u64_at(&call(&d, op::MKNOD, a, APP, &mknod).1, 0).unwrap();
    let mut rename = user.to_le_bytes().to_vec();
    rename.extend(cstr("a"));
    rename.extend(cstr("b"));
    assert_eq!(call(&d, op::RENAME, user, APP, &rename).0, 0);
    // The file's node follows its directory.
    assert_eq!(call(&d, op::GETATTR, f, APP, &[0; 16]).0, 0);
    assert!(dir.join("0/b/f").exists());
    // A rename that must not replace fails on an existing name.
    let mut noreplace = user.to_le_bytes().to_vec();
    noreplace.extend(RENAME_NOREPLACE.to_le_bytes());
    noreplace.extend([0; 4]);
    noreplace.extend(cstr("b"));
    noreplace.extend(cstr("Download"));
    assert_eq!(call(&d, op::RENAME2, user, APP, &noreplace).0, -errno::EEXIST);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn forgotten_nodes_go() {
    let dir = lower();
    let d = Daemon::new(&dir);
    let user = u64_at(&call(&d, op::LOOKUP, 1, APP, &cstr("0")).1, 0).unwrap();
    assert!(d.handle(&request(op::FORGET, user, APP, &1u64.to_le_bytes())).is_none());
    assert_eq!(call(&d, op::GETATTR, user, APP, &[0; 16]).0, -errno::ENOENT);
    fs::remove_dir_all(dir).unwrap();
}
