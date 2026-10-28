//! BPF_CGROUP_INET_SOCK_CREATE at inet_create/inet6_create, before publishing
//! the socket. A bounded interpreter for the image's read-only program; no
//! program/map names or permission policy live in this owner. Linux sources:
//! kernel/bpf/cgroup.c and net/ipv4/af_inet.c / net/ipv6/af_inet6.c.
use super::*;
use crate::errno::{EACCES, EOPNOTSUPP, Errno};
use std::path::Path;

const SOCK: u32 = 9;
pub(super) const ATTACH: u32 = 2;

fn io(error: std::io::Error) -> Errno {
    errno::from_darwin(error.raw_os_error().unwrap_or(libc::EIO))
}
fn open(path: &Path) -> Result<PrivateFile, Errno> {
    PrivateFile::allocate(|| std::fs::File::open(path)).map_err(io)
}

/// Resolve pseudo map FDs while the loader owns them. Object files last for
/// the boot, so their names retain map identity after close, unpin and exec.
pub(super) fn prepare(a: &[u32]) -> Result<Vec<u8>, i64> {
    if a[1] == 0 {
        return Err(-(EINVAL as i64));
    }
    if a[1] > 4096 {
        return Err(-(E2BIG as i64));
    }
    let pointer = a[2] as u64 | (a[3] as u64) << 32;
    let mut code = guest_bytes(pointer, a[1] as usize * 8)?;
    let mut refs = Vec::new();
    let mut pc = 0;
    while pc < code.len() {
        if code[pc] == 0x18 {
            if pc + 16 > code.len() {
                return Err(-(EINVAL as i64));
            }
            if code[pc + 1] >> 4 == 1 {
                let fd = i32::from_ne_bytes(code[pc + 4..pc + 8].try_into().unwrap());
                let pin = fdtab::pin_guest(fd).map_err(|e| -(e as i64))?;
                let fd = pin.descriptor().as_raw_fd();
                let (header, stat) = read_header(fd).ok_or(-(EINVAL as i64))?;
                if header.kind != KIND_MAP {
                    return Err(-(EINVAL as i64));
                }
                let path = object_path(fd).ok_or(-(EBADF as i64))?;
                let name = format!("map-{}-{}", stat.st_dev, stat.st_ino);
                match std::fs::hard_link(
                    std::ffi::OsStr::from_bytes(path.as_bytes()),
                    objects_dir().join(&name),
                ) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(e) => return Err(-(io(e) as i64)),
                }
                refs.extend_from_slice(&(name.len() as u32).to_ne_bytes());
                refs.extend_from_slice(name.as_bytes());
                // The reference table is indexed by instruction, not guest FD.
                code[pc + 4..pc + 8].copy_from_slice(&(pc as u32).to_ne_bytes());
            }
            pc += 8;
        }
        pc += 8;
    }
    code.extend(refs);
    Ok(code)
}

struct Program {
    code: Vec<u8>,
    maps: HashMap<usize, Kernel>,
}
impl Program {
    fn read(file: &PrivateFile) -> Result<Self, Errno> {
        let (h, _) = read_header(file.as_raw_fd()).ok_or(EINVAL)?;
        if h.kind != KIND_PROG || h.ty != SOCK || h.max_entries > 4096 {
            return Err(EINVAL);
        }
        let mut data = vec![0; h.data_len as usize];
        if unsafe { libc::pread(file.as_raw_fd(), data.as_mut_ptr().cast(), data.len(), 0) }
            != data.len() as isize
        {
            return Err(errno::EIO);
        }
        let size = h.max_entries as usize * 8;
        let code = data.get(..size).ok_or(EINVAL)?.to_vec();
        let mut refs = data.get(size..).ok_or(EINVAL)?;
        let mut maps = HashMap::new();
        for (pc, insn) in code.chunks_exact(8).enumerate() {
            if insn[0] != 0x18 || insn[1] >> 4 != 1 {
                continue;
            }
            let len = u32::from_ne_bytes(refs.get(..4).ok_or(EINVAL)?.try_into().unwrap()) as usize;
            let name = refs.get(4..4 + len).ok_or(EINVAL)?;
            refs = &refs[4 + len..];
            let path = objects_dir().join(std::ffi::OsStr::from_bytes(name));
            // Kernel map access needs the shared header's write lock even
            // for lookups. A read-only fd cannot back that writable mapping.
            let map = PrivateFile::allocate(|| {
                std::fs::File::options()
                    .read(true)
                    .write(true)
                    .custom_flags(libc::O_CLOEXEC)
                    .open(&path)
            })
            .map_err(io)?;
            let map = mapped(map.as_raw_fd()).map_err(|error| (-error) as Errno)?;
            maps.insert(pc * 8, Kernel(map));
        }
        let p = Self { code, maps };
        p.validate()?;
        Ok(p)
    }

    fn validate(&self) -> Result<(), Errno> {
        let count = self.code.len() / 8;
        let mut pc = 0;
        while pc < count {
            let i = &self.code[pc * 8..pc * 8 + 8];
            if i[1] & 15 > 10 || i[1] >> 4 > 10 {
                return Err(EINVAL);
            }
            let imm = i32::from_ne_bytes(i[4..8].try_into().unwrap());
            match i[0] {
                0x18 if pc + 1 < count && self.code[(pc + 1) * 8] == 0 => pc += 1,
                0x07 | 0x57 | 0x67 | 0x77 | 0x97 | 0xb7 | 0xbf | 0x63 | 0x71 | 0x95 => {}
                0x85 if matches!(imm, 1 | 15) => {}
                0x15 => {
                    let off = i16::from_ne_bytes(i[2..4].try_into().unwrap());
                    if off < 0 || pc + 1 + off as usize >= count {
                        return Err(EINVAL);
                    }
                }
                _ => return Err(EOPNOTSUPP),
            }
            pc += 1;
        }
        Ok(())
    }

    fn run(&self, uid: u32, gid: u32) -> Result<u64, Errno> {
        // Virtual offsets only: no register can address host/guest memory.
        // Offset 0 is NULL. r10 points one past the 512-byte BPF stack.
        let mut memory = vec![0u8; 513];
        let mut r = [0u64; 11];
        r[10] = 513;
        let mut pc = 0usize;
        for _ in 0..4096 {
            let i = self.code.get(pc * 8..pc * 8 + 8).ok_or(EINVAL)?;
            let (dst, src) = ((i[1] & 15) as usize, (i[1] >> 4) as usize);
            let off = i16::from_ne_bytes(i[2..4].try_into().unwrap()) as i64;
            let imm = i32::from_ne_bytes(i[4..8].try_into().unwrap()) as i64 as u64;
            pc += 1;
            match i[0] {
                0x18 => {
                    let hi = self.code.get(pc * 8 + 4..pc * 8 + 8).ok_or(EINVAL)?;
                    r[dst] = imm as u32 as u64
                        | (u32::from_ne_bytes(hi.try_into().unwrap()) as u64) << 32;
                    pc += 1;
                }
                0xb7 => r[dst] = imm,
                0xbf => r[dst] = r[src],
                0x07 => r[dst] = r[dst].wrapping_add(imm),
                0x57 => r[dst] &= imm,
                0x67 => r[dst] <<= imm & 63,
                0x77 => r[dst] >>= imm & 63,
                0x97 => {
                    if imm != 0 {
                        r[dst] %= imm;
                    }
                }
                0x63 | 0x71 => {
                    let at = r[dst_if_load(i[0], dst, src)].wrapping_add_signed(off) as usize;
                    if at == 0 {
                        return Err(EACCES);
                    }
                    if i[0] == 0x63 {
                        // STX W is permitted only on the BPF stack.
                        memory
                            .get_mut(at..at.checked_add(4).ok_or(EACCES)?)
                            .filter(|_| at + 4 <= 513)
                            .ok_or(EACCES)?
                            .copy_from_slice(&(r[src] as u32).to_ne_bytes());
                    } else {
                        r[dst] = *memory.get(at).ok_or(EACCES)? as u64;
                    }
                }
                0x15 if r[dst] == imm => pc += off as usize,
                0x15 => {}
                0x85 if imm == 15 => r[0] = uid as u64 | (gid as u64) << 32,
                0x85 if imm == 1 => {
                    let map = self.maps.get(&(r[1] as usize)).ok_or(EINVAL)?;
                    let at = usize::try_from(r[2]).map_err(|_| EACCES)?;
                    let len = map.0.h().key_size as usize;
                    if at == 0 {
                        return Err(EACCES);
                    }
                    let key = memory
                        .get(at..at.checked_add(len).ok_or(EACCES)?)
                        .ok_or(EACCES)?;
                    r[0] = match map.lookup(key) {
                        None => 0,
                        Some(value) => {
                            let pointer = memory.len() as u64;
                            memory.extend(value);
                            pointer
                        }
                    };
                }
                0x95 => return Ok(r[0]),
                _ => return Err(EINVAL),
            }
        }
        Err(E2BIG)
    }
}
fn dst_if_load(op: u8, dst: usize, src: usize) -> usize {
    if op == 0x71 { src } else { dst }
}

fn attachment(group: &Path, root: &Path) -> Result<PathBuf, Errno> {
    use std::os::macos::fs::MetadataExt;
    group.strip_prefix(root).map_err(|_| EBADF)?;
    let stat = std::fs::metadata(group).map_err(io)?;
    // Cgroup identity, rather than its reusable pathname. Birth time also
    // distinguishes a removed directory if Darwin reuses its inode.
    Ok(objects_dir().join("sock-create").join(format!(
        "{}-{}-{}-{}",
        stat.st_dev(),
        stat.st_ino(),
        stat.st_birthtime(),
        stat.st_birthtime_nsec()
    )))
}

/// flags=0 is Linux's single, non-overridable attachment mode. Other modes
/// are explicitly unsupported; they must not report a successful attachment.
pub(super) fn attach(a: &[u32], detach: bool) -> Result<(), Errno> {
    if a[3] & !7 != 0 {
        return Err(EINVAL);
    }
    if a[3] != 0 {
        return Err(EOPNOTSUPP);
    }
    let target = fdtab::pin_guest(a[0] as i32)?;
    let root = vfs::kernel_fs_root("cgroup2").ok_or(EBADF)?;
    let root = root.canonicalize().map_err(io)?;
    let path = object_path(target.descriptor().as_raw_fd()).ok_or(EBADF)?;
    let group = Path::new(std::ffi::OsStr::from_bytes(path.as_bytes()))
        .canonicalize()
        .map_err(io)?;
    if !group.is_dir() {
        return Err(EBADF);
    }
    let path = attachment(&group, &root)?;
    // Serialize replacement/hierarchy checks across processes; execution sees
    // either immutable program through the atomic rename.
    let lock = PrivateFile::allocate(|| {
        std::fs::File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(objects_dir().join("sock-create.lock"))
    })
    .map_err(io)?;
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(errno::last());
    }
    let program = if a[1] != 0 || !detach {
        Some(fdtab::pin_guest(a[1] as i32)?)
    } else {
        None
    };
    if detach {
        if let Some(p) = program {
            let (a, _) = read_header(p.descriptor().as_raw_fd()).ok_or(EINVAL)?;
            let (b, _) = read_header(open(&path)?.as_raw_fd()).ok_or(EINVAL)?;
            if a.id != b.id {
                return Err(ENOENT);
            }
        }
        return std::fs::remove_file(path).map_err(io);
    }
    let program = program.unwrap();
    let fd = program.descriptor().as_raw_fd();
    let (header, _) = read_header(fd).ok_or(EINVAL)?;
    let expected = u32::from_ne_bytes(header._pad[..4].try_into().unwrap());
    if header.ty != SOCK || (expected != 0 && expected != ATTACH) {
        return Err(EINVAL);
    }
    let file = open(Path::new(std::ffi::OsStr::from_bytes(
        object_path(fd).ok_or(EBADF)?.as_bytes(),
    )))?;
    Program::read(&file)?;
    for ancestor in group
        .ancestors()
        .skip(1)
        .take_while(|p| p.starts_with(&root))
    {
        if attachment(ancestor, &root)?.try_exists().map_err(io)? {
            return Err(EPERM);
        }
    }
    fn has_program(group: &Path, root: &Path) -> Result<bool, Errno> {
        for entry in std::fs::read_dir(group).map_err(io)? {
            let entry = entry.map_err(io)?;
            if entry.file_type().map_err(io)?.is_dir()
                && (attachment(&entry.path(), root)?.try_exists().map_err(io)?
                    || has_program(&entry.path(), root)?)
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
    if has_program(&group, &root)? {
        return Err(EPERM);
    }
    let dir = path.parent().unwrap();
    std::fs::create_dir_all(dir).map_err(io)?;
    let temporary = dir.join("next");
    let source = object_path(fd).ok_or(EBADF)?;
    // A failed previous replacement cannot leave an attachment in place.
    match std::fs::remove_file(&temporary) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(io(e)),
    }
    std::fs::hard_link(std::ffi::OsStr::from_bytes(source.as_bytes()), &temporary).map_err(io)?;
    std::fs::rename(temporary, path).map_err(io)
}

pub(crate) fn check() -> Result<(), Errno> {
    let Some(root) = vfs::kernel_fs_root("cgroup2") else {
        return Ok(());
    };
    let root = root.canonicalize().map_err(io)?;
    let membership = super::super::cgroup::proc_cgroup(0, super::super::thread::gettid() as i32);
    let group = membership
        .lines()
        .find_map(|line| line.strip_prefix("0::"))
        .ok_or(EINVAL)?;
    let group = root.join(group.trim_start_matches('/'));
    let (uid, gid) = super::super::attrs::ids(super::super::attrs::REAL);
    for group in group.ancestors().take_while(|p| p.starts_with(&root)) {
        let path = attachment(group, &root)?;
        let file = match open(&path) {
            Ok(file) => file,
            Err(ENOENT) => continue,
            Err(e) => return Err(e),
        };
        return if Program::read(&file)?.run(uid, gid)? == 1 {
            Ok(())
        } else {
            Err(EPERM)
        };
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sys::{cred, fs, net};

    fn call(command: u64, a: &[u32]) -> i64 {
        bpf([
            command,
            a.as_ptr() as u64,
            std::mem::size_of_val(a) as u64,
            0,
            0,
            0,
        ])
    }
    fn load(code: &[u8]) -> i64 {
        let mut a = [0; 24];
        a[0] = SOCK;
        a[1] = code.len() as u32 / 8;
        a[2] = code.as_ptr() as u64 as u32;
        a[3] = (code.as_ptr() as u64 >> 32) as u32;
        call(PROG_LOAD, &a)
    }
    fn close(fd: i64) {
        assert_eq!(fs::close([fd as u64, 0, 0, 0, 0, 0]), 0);
    }
    fn socket(family: u64, ty: u64) -> i64 {
        net::socket([family, ty, 0, 0, 0, 0])
    }
    fn setup() -> (std::sync::MutexGuard<'static, ()>, &'static Path, i64) {
        let (guard, dir) = vfs::test_view();
        let root = dir.join("data/cgroup");
        std::fs::create_dir_all(&root).unwrap();
        vfs::add_mount(
            "/sys/fs/cgroup",
            root,
            vfs::Area::Writable,
            "cgroup2",
            "cgroup2",
        )
        .unwrap();
        let fd = fs::openat([
            (-100i64) as u64,
            c"/sys/fs/cgroup".as_ptr() as u64,
            0o200000 | 0o2000000,
            0,
            0,
            0,
        ]);
        assert!(fd >= 0, "open cgroup: {fd}");
        (guard, dir, fd)
    }
    fn attach_at(target: i64, program: i64, detach: bool) -> i64 {
        call(
            if detach { PROG_DETACH } else { PROG_ATTACH },
            &[target as u32, program as u32, ATTACH, 0],
        )
    }
    fn constant(value: u8) -> Vec<u8> {
        vec![0xb7, 0, 0, 0, value, 0, 0, 0, 0x95, 0, 0, 0, 0, 0, 0, 0]
    }

    #[test]
    fn boot_mount_table_finds_cgroup_before_attaching_loader_pin() {
        if fdtab::isolated_kernel_test(
            "sys::bpf::sock_create::tests::boot_mount_table_finds_cgroup_before_attaching_loader_pin",
        ) {
            return;
        }
        let (_view, dir) = vfs::test_view();
        let runtime = vfs::runtime_dir().unwrap();
        let namespace = aim_storage::mount_namespace::Namespace::open(
            runtime,
            &vfs::mount_namespace_id().unwrap(),
        )
        .unwrap();
        let cgroup = dir.join("data/boot-cgroup");
        let bpf_root = dir.join("data/boot-bpf");
        std::fs::create_dir_all(&cgroup).unwrap();
        std::fs::create_dir_all(bpf_root.join("netd_shared")).unwrap();
        // guest-init's Layout publishes three-field path-map entries, not
        // mount(2) events with explicit source/type fields.
        let base = format!(
            "{}\ncgroup2\t/sys/fs/cgroup\t{}\nbpf\t/sys/fs/bpf\t{}\n",
            namespace.read().unwrap().base,
            cgroup.display(),
            bpf_root.display()
        );
        namespace.update_base(&base).unwrap();
        vfs::refresh_fuse_mounts().unwrap();
        let read = |path: &str| {
            let fd = crate::sys::procfs::open(path, 0, libc::O_RDONLY).unwrap();
            assert!(fd >= 0, "{path}: {fd}");
            let mut data = vec![0; 65536];
            let n = unsafe { libc::pread(fd as i32, data.as_mut_ptr().cast(), data.len(), 0) };
            assert!(n >= 0);
            data.truncate(n as usize);
            close(fd);
            String::from_utf8(data).unwrap()
        };
        let mounts = read("/proc/mounts");
        assert_eq!(mounts, read("/proc/self/mounts"));
        // This is how netd finds its attachment target. A directory that
        // exists but is reported as tmpfs is not a cgroup mount.
        let mount = mounts.lines().find_map(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            (fields[2] == "cgroup2").then_some(fields[1])
        });
        assert_eq!(mount, Some("/sys/fs/cgroup"), "{mounts}");
        assert!(mounts.contains("bpf /sys/fs/bpf bpf "), "{mounts}");
        let info = read("/proc/self/mountinfo");
        assert!(info.contains(" - cgroup2 none rw\n"), "{info}");
        assert!(info.contains(" - bpf bpf rw\n"), "{info}");
        let mount = CString::new(mount.unwrap()).unwrap();
        let target = fs::openat([
            vfs::LINUX_AT_FDCWD as u64,
            mount.as_ptr() as u64,
            0o200000,
            0,
            0,
            0,
        ]);
        assert!(target >= 0, "open cgroup: {target}");
        let path = c"/sys/fs/bpf/netd_shared/prog_netd_cgroupsock_inet_create";
        let get = [path.as_ptr() as u64, 0u64];
        assert_eq!(
            bpf([OBJ_GET, get.as_ptr() as u64, 16, 0, 0, 0]),
            -(ENOENT as i64)
        );
        let program = load(&constant(0));
        assert!(program >= 0);
        let pin = [path.as_ptr() as u64, program as u64];
        assert_eq!(bpf([OBJ_PIN, pin.as_ptr() as u64, 16, 0, 0, 0]), 0);
        close(program);
        // Loading and pinning precede attachment; neither enables policy.
        close(socket(2, 2));
        let program = bpf([OBJ_GET, get.as_ptr() as u64, 16, 0, 0, 0]);
        assert!(program >= 0, "reopen loader pin: {program}");
        assert_eq!(attach_at(target, program, false), 0);
        close(program);
        assert_eq!(socket(2, 2), -(EPERM as i64));
        assert_eq!(attach_at(target, 0, true), 0);
        close(target);
    }

    #[test]
    fn socket_create_attach_replace_detach_and_family_scope() {
        if let Some(dir) =
            std::env::args().find_map(|a| a.strip_prefix("bpf-shared-root=").map(PathBuf::from))
        {
            vfs::init(&dir.join("root"), Some(&dir.join("run/path-map"))).unwrap();
            assert_eq!(socket(2, 2), -(EPERM as i64));
            return;
        }
        if fdtab::isolated_kernel_test(
            "sys::bpf::sock_create::tests::socket_create_attach_replace_detach_and_family_scope",
        ) {
            return;
        }
        let (_view, _dir, target) = setup();
        let deny = load(&constant(0));
        assert!(deny >= 0);
        // A merely loaded program cannot affect sockets.
        close(socket(2, 2));
        assert_eq!(attach_at(target, deny, false), 0);
        // An exec'd process in the same boot must see this attachment too.
        use std::io::Write;
        writeln!(
            std::fs::OpenOptions::new()
                .append(true)
                .open(_dir.join("run/path-map"))
                .unwrap(),
            "cgroup2\t/sys/fs/cgroup\t{}",
            _dir.join("data/cgroup").display()
        )
        .unwrap();
        struct Child(std::process::Child);
        impl Drop for Child {
            fn drop(&mut self) {
                if self.0.try_wait().unwrap().is_none() {
                    self.0.kill().unwrap();
                }
                self.0.wait().unwrap();
            }
        }
        let mut child = Child(std::process::Command::new(std::env::current_exe().unwrap()).args([
            "--exact", "sys::bpf::sock_create::tests::socket_create_attach_replace_detach_and_family_scope",
            "--skip", "isolated-kernel-instance", "--skip", &format!("bpf-shared-root={}", _dir.display()),
        ]).spawn().unwrap());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        std::fs::create_dir(_dir.join("data/cgroup/child")).unwrap();
        let sub = fs::openat([
            (-100i64) as u64,
            c"/sys/fs/cgroup/child".as_ptr() as u64,
            0o200000,
            0,
            0,
            0,
        ]);
        assert!(sub >= 0);
        assert_eq!(attach_at(sub, deny, false), -(EPERM as i64));
        close(deny); // attachment, rather than the loader's FD, owns the program
        let before = fdtab::open_fds().len();
        for domain in [2, 10] {
            for ty in [1, 2, 2 | 0o4000 | 0o2000000] {
                assert_eq!(socket(domain, ty), -(EPERM as i64));
            }
        }
        assert_eq!(fdtab::open_fds().len(), before);
        close(socket(1, 1)); // UNIX sockets do not reach this hook
        assert_eq!(socket(0xffff, 2), -97);
        assert_eq!(socket(2, 5), -93);
        let allow = load(&constant(1));
        assert_eq!(attach_at(target, allow, false), 0);
        assert!(object_path(allow as i32).is_some(), "before socket");
        close(socket(2, 2));
        assert!(object_path(allow as i32).is_some(), "after socket4");
        close(socket(10, 2));
        assert!(object_path(allow as i32).is_some(), "after socket6");
        assert_eq!(attach_at(target, 0, true), 0);
        assert_eq!(attach_at(target, 0, true), -(ENOENT as i64));
        close(socket(2, 2));
        assert_eq!(attach_at(sub, allow, false), 0);
        assert_eq!(attach_at(target, allow, false), -(EPERM as i64));
        std::fs::remove_dir(_dir.join("data/cgroup/child")).unwrap();
        std::fs::create_dir(_dir.join("data/cgroup/child")).unwrap();
        // A new cgroup at the same pathname has no old attachment.
        assert_eq!(attach_at(target, allow, false), 0);
        assert_eq!(attach_at(target, 0, true), 0);
        close(sub);
        close(allow);
        // Non-cgroup target and wrong program FDs must not attach successfully.
        assert_eq!(attach_at(target, target, false), -(EINVAL as i64));
        assert_eq!(attach_at(-1, target, false), -(EBADF as i64));
        close(target);
    }

    fn image_sections(root: &Path) -> HashMap<String, Vec<u8>> {
        let elf = std::fs::read(root.join("apex/com.android.tethering/etc/bpf/netd_shared/netd.o"))
            .unwrap();
        let word = |at| u64::from_le_bytes(elf[at..at + 8].try_into().unwrap()) as usize;
        let half = |at| u16::from_le_bytes(elf[at..at + 2].try_into().unwrap()) as usize;
        let table = word(40);
        let strings = table + half(62) * 64;
        let names = &elf[word(strings + 24)..][..word(strings + 32)];
        let mut sections = HashMap::new();
        for i in 0..half(60) {
            let section = table + i * 64;
            let name = u32::from_le_bytes(elf[section..section + 4].try_into().unwrap()) as usize;
            let end = names[name..].iter().position(|b| *b == 0).unwrap() + name;
            sections.insert(
                String::from_utf8(names[name..end].to_vec()).unwrap(),
                elf[word(section + 24)..][..word(section + 32)].to_vec(),
            );
        }
        sections
    }

    fn image_program(root: &Path) -> Vec<u8> {
        image_sections(root)
            .remove("cgroupsock/inet_create")
            .unwrap()
    }

    #[test]
    fn netbpfload_image_pin_attaches_with_uncached_permission_map() {
        if fdtab::isolated_kernel_test(
            "sys::bpf::sock_create::tests::netbpfload_image_pin_attaches_with_uncached_permission_map",
        ) {
            return;
        }
        let Some(image) =
            aim_paths::original_image_with("apex/com.android.tethering/etc/bpf/netd_shared/netd.o")
        else {
            return;
        };
        let (_view, dir, target) = setup();
        std::fs::create_dir_all(dir.join("data/bpf/netd_shared")).unwrap();
        vfs::add_mount(
            "/sys/fs/bpf",
            dir.join("data/bpf"),
            vfs::Area::Writable,
            "bpf",
            "bpf",
        )
        .unwrap();
        let sections = image_sections(&image);
        let mut code = sections["cgroupsock/inet_create"].clone();
        // NetBpfLoad::applyRelo resolves the ELF relocation's map symbol,
        // creates its real definition and writes BPF_PSEUDO_MAP_FD + fd.
        let rel = &sections[".relcgroupsock/inet_create"];
        assert_eq!(rel.len(), 16);
        let offset = u64::from_le_bytes(rel[..8].try_into().unwrap()) as usize;
        let info = u64::from_le_bytes(rel[8..].try_into().unwrap());
        assert_eq!(info as u32, 1); // R_BPF_64_64
        let sym = &sections[".symtab"][(info >> 32) as usize * 24..][..24];
        let name = u32::from_le_bytes(sym[..4].try_into().unwrap()) as usize;
        assert!(sections[".strtab"][name..].starts_with(b"uid_permission_map\0"));
        let value = u64::from_le_bytes(sym[8..16].try_into().unwrap()) as usize;
        let def = &sections["maps"][value..];
        // The image's loader passes sizeof(bpf_attr)=152 (also visible at
        // its BPF_PROG_LOAD syscall site), with all unused fields zero.
        let mut attr = [0u32; 38];
        for (word, bytes) in attr[..5].iter_mut().zip(def.chunks_exact(4)) {
            *word = u32::from_le_bytes(bytes.try_into().unwrap());
        }
        assert_eq!(&attr[..5], &[HASH, 4, 1, 4000, 0]);
        for (word, bytes) in attr[7..11]
            .iter_mut()
            .zip(b"uid_permission_\0".chunks_exact(4))
        {
            *word = u32::from_ne_bytes(bytes.try_into().unwrap());
        }
        let map = call(MAP_CREATE, &attr);
        assert!(map >= 0, "BPF_MAP_CREATE: {map}");
        // Do not map/lookup/update it in this process before attachment:
        // the loader and netd have separate in-process mapping caches.
        let map_path = c"/sys/fs/bpf/netd_shared/map_netd_uid_permission_map";
        let ptr = |p: *const u8| [p as u64 as u32, (p as u64 >> 32) as u32];
        attr.fill(0);
        attr[..2].copy_from_slice(&ptr(map_path.as_ptr().cast()));
        attr[2] = map as u32;
        assert_eq!(call(OBJ_PIN, &attr), 0);
        assert_eq!(code[offset], 0x18);
        code[offset + 1] = (code[offset + 1] & 15) | 0x10;
        code[offset + 4..offset + 8].copy_from_slice(&(map as i32).to_ne_bytes());
        let license = &sections["license"];
        let mut log = vec![0u8; 1 << 20];
        attr.fill(0);
        attr[0] = SOCK;
        attr[1] = code.len() as u32 / 8;
        attr[2..4].copy_from_slice(&ptr(code.as_ptr()));
        attr[4..6].copy_from_slice(&ptr(license.as_ptr()));
        attr[6] = 1;
        attr[7] = log.len() as u32;
        attr[8..10].copy_from_slice(&ptr(log.as_mut_ptr()));
        attr[10] = (6 << 24) | (12 << 16); // NetBpfLoad's kernelVersion()
        let name = b"cgroupsock_inet\0";
        for (word, bytes) in attr[12..16].iter_mut().zip(name.chunks_exact(4)) {
            *word = u32::from_ne_bytes(bytes.try_into().unwrap());
        }
        // NetBpfLoad's cgroupsock/ entry leaves expected_attach_type=0.
        assert_eq!(attr[17], 0);
        let program = call(PROG_LOAD, &attr);
        assert!(program >= 0, "NetBpfLoad BPF_PROG_LOAD: {program}");
        let path = c"/sys/fs/bpf/netd_shared/prog_netd_cgroupsock_inet_create";
        attr.fill(0);
        attr[..2].copy_from_slice(&ptr(path.as_ptr().cast()));
        attr[2] = program as u32;
        assert_eq!(call(OBJ_PIN, &attr), 0);
        close(program);
        close(map);
        let get = [path.as_ptr() as u64, 0u64];
        let program = bpf([OBJ_GET, get.as_ptr() as u64, 16, 0, 0, 0]);
        assert!(program >= 0, "netd BPF_OBJ_GET: {program}");
        let held = fdtab::pin_guest(target as i32).unwrap();
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        assert_eq!(
            unsafe { libc::fstat(held.descriptor().as_raw_fd(), &mut stat) },
            0
        );
        assert_eq!(stat.st_mode & libc::S_IFMT, libc::S_IFDIR);
        drop(held);
        assert!(
            MAPPED
                .lock()
                .unwrap()
                .as_ref()
                .is_none_or(HashMap::is_empty)
        );
        assert_eq!(attach_at(target, program, false), 0, "netd BPF_PROG_ATTACH");
        close(program);
        // Original BpfHandler uses BPF_F_RDONLY and a full attr union;
        // compact zero-extended attrs used by replacement netd are valid too.
        attr.fill(0);
        attr[..2].copy_from_slice(&ptr(path.as_ptr().cast()));
        attr[3] = 8; // BPF_F_RDONLY
        let program = call(OBJ_GET, &attr);
        assert!(program >= 0, "BpfHandler BPF_OBJ_GET: {program}");
        attr.fill(0);
        attr[..4].copy_from_slice(&[target as u32, program as u32, ATTACH, 0]);
        assert_eq!(call(PROG_ATTACH, &attr), 0, "BpfHandler BPF_PROG_ATTACH");
        close(program);
        // Attachment owns the program/map, and the actual image code sees
        // both an empty map and subsequent policy writes.
        close(socket(2, 2));
        let get = [map_path.as_ptr() as u64, 0u64];
        let map = bpf([OBJ_GET, get.as_ptr() as u64, 16, 0, 0, 0]);
        assert!(map >= 0);
        let uid = cred::current().uid[0] % 100000;
        let kernel = Kernel::of(map as i32).unwrap();
        assert!(kernel.update(&uid.to_ne_bytes(), |v| v[0] = 0));
        assert_eq!(socket(2, 2), -(EPERM as i64));
        assert!(kernel.update(&uid.to_ne_bytes(), |v| v[0] = 4));
        close(socket(10, 1));
        assert_eq!(attach_at(target, 0, true), 0);
        close(map);
        close(target);
    }

    #[test]
    fn loaded_image_permissions_use_real_uid_live_map_and_retained_map_identity() {
        if fdtab::isolated_kernel_test(
            "sys::bpf::sock_create::tests::loaded_image_permissions_use_real_uid_live_map_and_retained_map_identity",
        ) {
            return;
        }
        let Some(image) =
            aim_paths::original_image_with("apex/com.android.tethering/etc/bpf/netd_shared/netd.o")
        else {
            return;
        };
        let (_view, _dir, target) = setup();
        let map = call(MAP_CREATE, &[HASH, 4, 1, 16, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert!(map >= 0);
        let kernel = Kernel::of(map as i32).unwrap();
        let name = CString::new("/data/map-pin").unwrap();
        let pin = [name.as_ptr() as u64, map as u64, 0];
        assert_eq!(bpf([OBJ_PIN, pin.as_ptr() as u64, 24, 0, 0, 0]), 0);
        close(map);
        let get = [name.as_ptr() as u64, 0, 0];
        let map = bpf([OBJ_GET, get.as_ptr() as u64, 24, 0, 0, 0]);
        assert!(map >= 0);
        let mut code = image_program(&image);
        let mut references = 0;
        for i in code.chunks_exact_mut(8) {
            if i[0] == 0x18 {
                i[1] |= 0x10; // Linux loader's BPF_PSEUDO_MAP_FD relocation
                i[4..8].copy_from_slice(&(map as i32).to_ne_bytes());
                references += 1;
            }
        }
        assert_eq!(references, 1);
        let program = load(&code);
        assert!(program >= 0, "load: {program}");
        assert_eq!(attach_at(target, program, false), 0);
        close(program);
        close(map);
        std::fs::remove_file(_dir.join("data/map-pin")).unwrap();
        // Another map, irrespective of any FD reuse, cannot replace this map.
        let unrelated = call(MAP_CREATE, &[HASH, 4, 1, 16, 0, 0, 0, 0, 0, 0, 0, 0]);
        Kernel::of(unrelated as i32)
            .unwrap()
            .update(&10123u32.to_ne_bytes(), |v| v[0] = 4);
        let mut identity = cred::current();
        for uid in [10123, 110123] {
            identity.uid = [uid, 0, 0, 0]; // real UID differs from effective/fs UID
            cred::init(identity.clone(), None);
            if uid == 10123 {
                close(socket(2, 2));
            } // missing entry defaults to INTERNET
            assert!(kernel.update(&10123u32.to_ne_bytes(), |v| v[0] = 0));
            for permission in [0, 1, 2, 3, 8, 0xff & !4] {
                assert!(kernel.update(&10123u32.to_ne_bytes(), |v| v[0] = permission));
                assert_eq!(socket(2, 2), -(EPERM as i64));
                assert_eq!(socket(10, 1), -(EPERM as i64));
            }
            assert!(kernel.update(&10123u32.to_ne_bytes(), |v| v[0] = 4));
            let existing = socket(2, 2);
            assert!(existing >= 0);
            assert!(kernel.update(&10123u32.to_ne_bytes(), |v| v[0] = 0));
            // Creation policy does not retroactively revoke an open socket.
            let address = [2u8, 0, 0, 0, 127, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0];
            assert_eq!(
                net::bind([existing as u64, address.as_ptr() as u64, 16, 0, 0, 0]),
                0
            );
            close(existing);
        }
        // The executed program is read-only: no other UID permission appears.
        assert_eq!(kernel.lookup(&0u32.to_ne_bytes()), None);
        assert_eq!(attach_at(target, 0, true), 0);
        close(socket(2, 2));
        close(unrelated);
        close(target);
    }
}
