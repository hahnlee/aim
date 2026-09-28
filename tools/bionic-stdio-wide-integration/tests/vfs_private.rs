use bionic_fs_facade::{
    aim_bionic_fs_process_install, aim_bionic_fs_process_uninstall, aim_bionic_fs_read_core,
};
use bionic_stdio_facade::{
    AndroidFile, aim_bionic_stdio_fclose_core, aim_bionic_stdio_fileno_core,
    aim_bionic_stdio_fopen_core, aim_bionic_stdio_fread_core, aim_bionic_stdio_fseek_core,
    aim_bionic_stdio_ftello_core, aim_bionic_stdio_fwrite_core, aim_bionic_stdio_process_install,
    aim_bionic_stdio_process_uninstall,
};
use std::ffi::{CString, c_void};
use std::fs::{self, File};
use std::os::fd::AsRawFd;

unsafe fn open(path: &str, mode: &str) -> *mut AndroidFile {
    let path = CString::new(path).unwrap();
    let mode = CString::new(mode).unwrap();
    // SAFETY: both CString buffers stay live for this synchronous call.
    unsafe { aim_bionic_stdio_fopen_core(path.as_ptr(), mode.as_ptr()) }
}

unsafe fn write(file: *mut AndroidFile, bytes: &[u8]) {
    // SAFETY: the stream is live and bytes are readable for their length.
    assert_eq!(
        unsafe {
            aim_bionic_stdio_fwrite_core(bytes.as_ptr().cast::<c_void>(), 1, bytes.len(), file)
        },
        bytes.len()
    );
}

unsafe fn read(file: *mut AndroidFile, bytes: &mut [u8]) {
    // SAFETY: the stream is live and bytes are writable for their length.
    assert_eq!(
        unsafe {
            aim_bionic_stdio_fread_core(bytes.as_mut_ptr().cast::<c_void>(), 1, bytes.len(), file)
        },
        bytes.len()
    );
}

#[test]
fn private_vfs_streams_persist_seek_append_and_share_offsets() {
    // Claim the directory before creating its contents.  This avoids reusing
    // a stale PID-only directory (and, importantly, never removes one owned
    // by another test/process).
    let temp_dir = std::env::temp_dir();
    let prefix = format!("aim-stdio-vfs-{}", std::process::id());
    let root = (0..1000)
        .find_map(|attempt| {
            let candidate = temp_dir.join(format!("{prefix}-{attempt}"));
            match fs::create_dir(&candidate) {
                Ok(()) => Some(candidate),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => None,
                Err(error) => panic!("failed to claim temporary VFS root: {error}"),
            }
        })
        .expect("could not claim a unique temporary VFS root");
    fs::create_dir_all(root.join("user/0/test/files")).unwrap();
    // SAFETY: this test installs the process-local VFS before opening streams.
    unsafe { std::env::set_var("AIM_ANDROID_PRIVATE_DATA_ROOT", &root) };
    let authority = File::open("/").unwrap();
    // SAFETY: pointers and lengths refer to static paths for this call.
    assert_eq!(
        unsafe {
            aim_bionic_fs_process_install(
                authority.as_raw_fd(),
                c"/".as_ptr().cast(),
                1,
                c"/".as_ptr().cast(),
                1,
            )
        },
        0
    );
    // Exercise the production process owner.  This selects
    // `allow_memory_files = false`, so failed VFS opens cannot be hidden by
    // the unit-test fixture table.
    assert_eq!(aim_bionic_stdio_process_install(), 0);

    // A real VFS failure must not fall through to the bounded in-memory
    // fixture path: r+ requires an existing file, and /system is immutable.
    let missing = unsafe { open("/data/user/0/test/files/missing", "r+") };
    assert!(missing.is_null(), "r+ unexpectedly created a missing file");
    let read_only = unsafe { open("/system/stdio-write-forbidden", "w") };
    assert!(
        read_only.is_null(),
        "w unexpectedly opened an immutable path"
    );

    let path = "/data/user/0/test/files/persist";

    let file = unsafe { open(path, "w") };
    assert!(!file.is_null());
    unsafe { write(file, b"persist") };
    assert_eq!(unsafe { aim_bionic_stdio_fclose_core(file) }, 0);
    let file = unsafe { open(path, "r") };
    let mut bytes = [0; 7];
    unsafe { read(file, &mut bytes) };
    assert_eq!(&bytes, b"persist");
    assert_eq!(unsafe { aim_bionic_stdio_fclose_core(file) }, 0);

    let file = unsafe { open(path, "r+") };
    assert_eq!(unsafe { aim_bionic_stdio_fseek_core(file, 3, 0) }, 0);
    let position_before_failed_seek = aim_bionic_stdio_ftello_core(file);
    assert_ne!(
        unsafe { aim_bionic_stdio_fseek_core(file, -100, 2) },
        0,
        "SEEK_END before the beginning unexpectedly succeeded"
    );
    assert_eq!(
        aim_bionic_stdio_ftello_core(file),
        position_before_failed_seek,
        "failed seek changed the stream position"
    );
    assert_eq!(unsafe { aim_bionic_stdio_fseek_core(file, 1, 0) }, 0);
    unsafe { write(file, b"X") };
    assert_eq!(unsafe { aim_bionic_stdio_fclose_core(file) }, 0);
    let file = unsafe { open(path, "r") };
    let mut bytes = [0; 7];
    unsafe { read(file, &mut bytes) };
    assert_eq!(&bytes, b"pXrsist");
    assert_eq!(unsafe { aim_bionic_stdio_fclose_core(file) }, 0);

    let file = unsafe { open(path, "a+") };
    assert_eq!(unsafe { aim_bionic_stdio_fseek_core(file, 0, 0) }, 0);
    unsafe { write(file, b"!") };
    assert_eq!(unsafe { aim_bionic_stdio_fclose_core(file) }, 0);
    let file = unsafe { open(path, "r") };
    let mut bytes = [0; 8];
    unsafe { read(file, &mut bytes) };
    assert_eq!(&bytes, b"pXrsist!");
    assert_eq!(unsafe { aim_bionic_stdio_fclose_core(file) }, 0);

    let sparse_path = "/data/user/0/test/files/sparse";
    let file = unsafe { open(sparse_path, "w+") };
    let sparse_offset = 256_i64 * 1024 * 1024 + 4095;
    assert_eq!(
        unsafe { aim_bionic_stdio_fseek_core(file, sparse_offset, 0) },
        0
    );
    unsafe { write(file, b"S") };
    let fd = aim_bionic_stdio_fileno_core(file);
    assert!(fd >= 10_000);
    assert_eq!(
        unsafe { aim_bionic_stdio_fseek_core(file, sparse_offset, 0) },
        0
    );
    let mut one = [0; 1];
    unsafe { read(file, &mut one) };
    assert_eq!(one, [b'S']);
    assert_eq!(unsafe { aim_bionic_stdio_fclose_core(file) }, 0);

    let shared_path = "/data/user/0/test/files/shared";
    let file = unsafe { open(shared_path, "w+") };
    unsafe { write(file, b"hello") };
    let fd = aim_bionic_stdio_fileno_core(file);
    assert!(fd >= 10_000);
    assert_eq!(aim_bionic_stdio_ftello_core(file), 5);
    assert_eq!(unsafe { aim_bionic_stdio_fseek_core(file, 0, 0) }, 0);
    let mut direct = [0; 2];
    // SAFETY: direct VFS read uses the descriptor owned by the live FILE.
    assert_eq!(
        unsafe { aim_bionic_fs_read_core(fd, direct.as_mut_ptr().cast(), direct.len()) },
        2
    );
    assert_eq!(&direct, b"he");
    // The external VFS read and FILE's ftello observe the same open-file
    // description offset, rather than a cached stdio-only cursor.
    assert_eq!(aim_bionic_stdio_ftello_core(file), 2);
    assert_eq!(unsafe { aim_bionic_stdio_fclose_core(file) }, 0);

    assert_eq!(aim_bionic_stdio_process_uninstall(), 0);
    assert_eq!(aim_bionic_fs_process_uninstall(), 0);
    fs::remove_dir_all(root).unwrap();
}
