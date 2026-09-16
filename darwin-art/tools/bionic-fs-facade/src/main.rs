use bionic_fs_facade::{
    IOCTL_FD_BAD, IOCTL_FD_FOUND, IOCTL_FD_INFO_ABI_VERSION, IOCTL_FD_OTHER,
    IOCTL_FD_RANDOM_DEVICE, IoctlFdInfo, PROCESS_OWNER_OK, SENDFILE_ABI_VERSION,
    SENDFILE_TRANSFER_OK, SendfileRequest, SendfileResult, darwin_art_bionic_fs_close_core,
    darwin_art_bionic_fs_ioctl_fd_lookup, darwin_art_bionic_fs_open_core,
    darwin_art_bionic_fs_process_has_capability_failure, darwin_art_bionic_fs_process_install,
    darwin_art_bionic_fs_process_uninstall, darwin_art_bionic_fs_read_core,
    darwin_art_bionic_fs_seed_private_directory, darwin_art_bionic_fs_sendfile_transfer,
};
use darwin_art_elf_loader::{
    LoadedElf, ResolveError, ResolvedSymbol, SymbolRequest, SymbolResolver,
};
use std::env;
use std::ffi::{c_char, c_int, c_void};
use std::fs::{self, File};
use std::num::NonZeroUsize;
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::process::ExitCode;

struct ProcessOwnerGuard(bool);

impl Drop for ProcessOwnerGuard {
    fn drop(&mut self) {
        if self.0 {
            let _ = darwin_art_bionic_fs_process_uninstall();
        }
    }
}

unsafe extern "C" {
    fn __error() -> *mut i32;
    fn darwin_art_bionic_fs_resolve(name: *const c_char) -> Option<unsafe extern "C" fn()>;
    fn darwin_art_bionic_errno_resolve(name: *const c_char) -> Option<unsafe extern "C" fn()>;
    #[link_name = "fpathconf"]
    fn host_fpathconf(fd: c_int, name: c_int) -> i64;
    #[link_name = "fstatvfs"]
    fn host_fstatvfs(fd: c_int, status: *mut HostStatvfs) -> c_int;
}

#[repr(C)]
#[derive(Default)]
struct HostStatvfs {
    f_bsize: u64,
    f_frsize: u64,
    f_blocks: u64,
    f_bfree: u64,
    f_bavail: u64,
    f_files: u64,
    f_ffree: u64,
    f_favail: u64,
    f_fsid: u64,
    f_flag: u64,
    f_namemax: u64,
}

const _: () = assert!(size_of::<HostStatvfs>() == 88);

const DARWIN_PC_2_SYMLINKS: c_int = 15;
const PORTABLE_ST_FLAGS: u64 = 0x0003;
const ANDROID_ST_RDONLY: u64 = 0x0001;

struct ClosedResolver;

impl ClosedResolver {
    fn resolved(address: unsafe extern "C" fn()) -> ResolvedSymbol {
        let address = NonZeroUsize::new(address as usize).expect("function address is non-zero");
        // SAFETY: each provider is manifest-checked against its Android arm64 import ABI
        // and remains linked for the loaded image lifetime.
        unsafe { ResolvedSymbol::new(address) }
    }

    fn lookup(name: &str) -> Option<unsafe extern "C" fn()> {
        let bytes = match name {
            "__errno" => c"__errno",
            "chdir" => c"chdir",
            "close" => c"close",
            "closedir" => c"closedir",
            "dirfd" => c"dirfd",
            "fchmod" => c"fchmod",
            "fchmodat" => c"fchmodat",
            "fdopendir" => c"fdopendir",
            "fstat" => c"fstat",
            "fstatat" => c"fstatat",
            "ftruncate" => c"ftruncate",
            "getcwd" => c"getcwd",
            "isatty" => c"isatty",
            "link" => c"link",
            "lstat" => c"lstat",
            "mkdir" => c"mkdir",
            "mkdirat" => c"mkdirat",
            "open" => c"open",
            "openat" => c"openat",
            "opendir" => c"opendir",
            "pathconf" => c"pathconf",
            "read" => c"read",
            "readdir" => c"readdir",
            "readlink" => c"readlink",
            "realpath" => c"realpath",
            "remove" => c"remove",
            "rename" => c"rename",
            "stat" => c"stat",
            "statvfs" => c"statvfs",
            "symlink" => c"symlink",
            "truncate" => c"truncate",
            "unlinkat" => c"unlinkat",
            "utimensat" => c"utimensat",
            _ => return None,
        };
        // SAFETY: both closed C resolvers read only the static NUL-terminated name.
        unsafe {
            darwin_art_bionic_fs_resolve(bytes.as_ptr())
                .or_else(|| darwin_art_bionic_errno_resolve(bytes.as_ptr()))
        }
    }
}

impl SymbolResolver for ClosedResolver {
    fn resolve(
        &mut self,
        request: SymbolRequest<'_>,
    ) -> Result<Option<ResolvedSymbol>, ResolveError> {
        if request.version.is_some() {
            return Err(ResolveError::Rejected(
                "filesystem fixture imports must be unversioned".to_owned(),
            ));
        }
        Ok(Self::lookup(request.symbol).map(Self::resolved))
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = env::args_os();
    let _program = arguments.next();
    let fixture = arguments
        .next()
        .ok_or("missing Android filesystem fixture")?;
    let root = arguments.next().ok_or("missing mount root directory")?;
    if arguments.next().is_some() {
        return Err("usage: bionic-fs-facade <android-arm64-fixture.so> <mount-root>".into());
    }
    let metadata = fs::metadata(&root)?;
    if !metadata.is_dir() {
        return Err("mount root is not a directory".into());
    }
    let root = PathBuf::from(root);
    let root_authority = File::open(&root)?;
    // SAFETY: root_authority and both immutable byte slices remain live until
    // the synchronous call has duplicated the trusted directory fd.
    let install_status = unsafe {
        darwin_art_bionic_fs_process_install(
            root_authority.as_raw_fd(),
            b"/system".as_ptr(),
            b"/system".len(),
            b"/system".as_ptr(),
            b"/system".len(),
        )
    };
    if install_status != PROCESS_OWNER_OK {
        return Err(format!("process owner install failed: {install_status}").into());
    }
    let mut process_owner = ProcessOwnerGuard(true);
    // SAFETY: the static app-data path is a readable NUL-terminated string.
    if unsafe { darwin_art_bionic_fs_seed_private_directory(c"/data/user/0/probe".as_ptr()) } != 0 {
        return Err("private app directory seed failed".into());
    }
    let guest_thread = std::thread::spawn(|| {
        // SAFETY: static path and local output remain live through each call.
        let fd = unsafe { darwin_art_bionic_fs_open_core(c"/dev/random".as_ptr(), 0, 0) };
        if fd < 0 {
            return false;
        }
        let mut bytes = [0_u8; 16];
        let read =
            unsafe { darwin_art_bionic_fs_read_core(fd, bytes.as_mut_ptr().cast(), bytes.len()) };
        let mut info = IoctlFdInfo::default();
        let lookup =
            unsafe { darwin_art_bionic_fs_ioctl_fd_lookup(std::ptr::null_mut(), fd, &mut info) };
        read == bytes.len() as isize
            && lookup == IOCTL_FD_FOUND
            && info.kind == IOCTL_FD_RANDOM_DEVICE
            && darwin_art_bionic_fs_close_core(fd) == 0
    });
    if !guest_thread.join().map_err(|_| "guest pthread panicked")? {
        return Err("process-wide guest pthread filesystem access failed".into());
    }
    // SAFETY: static C strings satisfy the Android path ABI and the returned
    // virtual descriptors remain owned by this active facade until close.
    let random_fd = unsafe { darwin_art_bionic_fs_open_core(c"/dev/random".as_ptr(), 0, 0) };
    if random_fd < 0 {
        return Err("synthetic /dev/random open failed".into());
    }
    let mut info = IoctlFdInfo::default();
    // SAFETY: info is a writable callback record on this activated pthread.
    if unsafe { darwin_art_bionic_fs_ioctl_fd_lookup(std::ptr::null_mut(), random_fd, &mut info) }
        != IOCTL_FD_FOUND
        || info.abi_version != IOCTL_FD_INFO_ABI_VERSION
        || info.kind != IOCTL_FD_RANDOM_DEVICE
    {
        return Err("random ioctl fd classification drift".into());
    }
    if darwin_art_bionic_fs_close_core(random_fd) != 0 {
        return Err("synthetic /dev/random close failed".into());
    }
    // SAFETY: same writable callback record; the closed token must be absent.
    if unsafe { darwin_art_bionic_fs_ioctl_fd_lookup(std::ptr::null_mut(), random_fd, &mut info) }
        != IOCTL_FD_BAD
    {
        return Err("closed random fd remained classified".into());
    }
    // SAFETY: exact synthetic path and fixed-register O_RDONLY form.
    let urandom_fd = unsafe { darwin_art_bionic_fs_open_core(c"/dev/urandom".as_ptr(), 0, 0) };
    if urandom_fd != random_fd {
        return Err("closed synthetic descriptor was not safely reused".into());
    }
    if darwin_art_bionic_fs_close_core(urandom_fd) != 0 {
        return Err("synthetic /dev/urandom close failed".into());
    }
    // SAFETY: exact brokered path and fixed-register O_RDONLY form.
    let regular_fd =
        unsafe { darwin_art_bionic_fs_open_core(c"/system/etc/payload.txt".as_ptr(), 0, 0) };
    if regular_fd != random_fd {
        return Err("descriptor reuse across kinds drifted".into());
    }
    // SAFETY: same callback record on the active pthread.
    if unsafe { darwin_art_bionic_fs_ioctl_fd_lookup(std::ptr::null_mut(), regular_fd, &mut info) }
        != IOCTL_FD_FOUND
        || info.kind != IOCTL_FD_OTHER
    {
        return Err("regular ioctl fd classification drift".into());
    }
    // libc++ copy_file opens this private-prefix output and supplies the exact
    // exact out/in/null-offset/remaining shape through the sendfile seam.
    let output_fd = unsafe {
        darwin_art_bionic_fs_open_core(c"/data/libcxx-copy".as_ptr(), 1 | 64 | 512, 0o644)
    };
    if output_fd < 0 {
        return Err("private /data output open failed".into());
    }
    let request = SendfileRequest {
        abi_version: SENDFILE_ABI_VERSION,
        output_fd,
        input_fd: regular_fd,
        has_explicit_offset: 0,
        offset: 0,
        count: 128,
    };
    let mut transfer = SendfileResult::default();
    if unsafe {
        darwin_art_bionic_fs_sendfile_transfer(std::ptr::null_mut(), &request, &mut transfer)
    } != SENDFILE_TRANSFER_OK
        || transfer.transferred != 13
        || transfer.android_errno != 0
    {
        return Err("private /data sendfile transfer failed".into());
    }
    if darwin_art_bionic_fs_close_core(output_fd) != 0
        || darwin_art_bionic_fs_close_core(regular_fd) != 0
    {
        return Err("brokered copy descriptors close failed".into());
    }
    let bytes = fs::read(fixture)?;
    let mut resolver = ClosedResolver;
    let image = LoadedElf::load_with_resolver(&bytes, &mut resolver)?;
    image.run_initializers()?;
    let host_file = File::open(root.join("etc/payload.txt"))?;
    // SAFETY: the descriptor is live and Darwin selector 15 is the semantic
    // _PC_2_SYMLINKS reference. Android selector 7 reaches the translated path.
    let host_symlinks = unsafe { host_fpathconf(host_file.as_raw_fd(), DARWIN_PC_2_SYMLINKS) };
    let guest_symlinks = image.call_exported_i32("bionic_fs_fixture_pc_2_symlinks")?;
    if i64::from(guest_symlinks) != host_symlinks {
        return Err(format!(
            "pathconf semantic translation drift: guest={guest_symlinks} host={host_symlinks}"
        )
        .into());
    }
    let mut host_filesystem = HostStatvfs::default();
    // SAFETY: the live descriptor and writable Darwin statvfs record match the
    // local SDK ABI compiled into this native executable.
    if unsafe { host_fstatvfs(host_file.as_raw_fd(), &mut host_filesystem) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let guest_bsize = image.call_exported_i32("bionic_fs_fixture_statvfs_bsize")?;
    let guest_flags = image.call_exported_i32("bionic_fs_fixture_statvfs_flags")?;
    let expected_flags = (host_filesystem.f_flag & PORTABLE_ST_FLAGS) | ANDROID_ST_RDONLY;
    if u64::try_from(guest_bsize).ok() != Some(host_filesystem.f_bsize)
        || u64::try_from(guest_flags).ok() != Some(expected_flags)
    {
        return Err("statvfs Darwin-to-Android differential drift".into());
    }
    // SAFETY: Darwin __error returns this host pthread's errno cell. It is never
    // exposed to the guest resolver and is used only to audit shim isolation.
    unsafe { *__error() = 33_001 };
    let result = image.call_exported_i32("bionic_fs_fixture_run")?;
    if result != 42 {
        return Err(format!("Android filesystem fixture failed at step {result}").into());
    }
    if darwin_art_bionic_fs_process_has_capability_failure() != 0 {
        return Err("facade encountered an untranslatable host capability failure".into());
    }
    // SAFETY: same host-only audit cell set immediately before guest execution.
    if unsafe { *__error() } != 33_001 {
        return Err("filesystem facade changed host errno".into());
    }
    if darwin_art_bionic_fs_process_uninstall() != PROCESS_OWNER_OK {
        return Err("process owner uninstall failed".into());
    }
    process_owner.0 = false;
    println!(
        "bionic-fs-facade: PASS Android ELF file/path/cwd/DIR+fdopendir mount-root broker errno"
    );
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("bionic-fs-facade: {error}");
            ExitCode::from(2)
        }
    }
}

#[allow(dead_code)]
fn _function_pointer_is_data_pointer_sized() {
    const _: () = assert!(size_of::<unsafe extern "C" fn()>() == size_of::<*mut c_void>());
}
