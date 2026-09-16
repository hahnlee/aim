#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Barrier, Condvar};
    use std::thread;
    use std::time::Duration;

    static PROCESS_TEST_LOCK: Mutex<()> = Mutex::new(());

    unsafe extern "C" {
        fn __error() -> *mut i32;
        fn darwin_art_bionic_errno_load() -> i32;
    }

    unsafe extern "C" fn open_binder(flags: c_int, mode: u32, error: *mut c_int) -> c_int {
        assert_eq!(flags, O_RDWR | O_CLOEXEC);
        assert_eq!(mode, 0);
        unsafe { *error = 0 };
        0x4000_0123
    }

    unsafe extern "C" fn duplicate_host_descriptor(
        fd: c_int,
        host_fd: *mut c_int,
    ) -> c_int {
        if host_fd.is_null() {
            return -1;
        }
        // SAFETY: this focused callback is invoked with one live test fd and
        // transfers the successful duplicate to the facade.
        let duplicate = unsafe { BorrowedFd::borrow_raw(fd) }.try_clone_to_owned();
        match duplicate {
            Ok(duplicate) => {
                unsafe { host_fd.write(duplicate.into_raw_fd()) };
                1
            }
            Err(_) => -1,
        }
    }

    #[test]
    fn fstat_borrows_metadata_from_an_external_descriptor_owner() {
        let _guard = PROCESS_TEST_LOCK.lock().unwrap();
        assert_eq!(
            darwin_art_bionic_fs_bind_host_descriptor_resolver(Some(
                duplicate_host_descriptor
            )),
            0
        );
        let path = std::env::temp_dir().join(format!(
            "darwin-art-external-fstat-{}",
            std::process::id()
        ));
        let external = OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        external.set_len(65_536).unwrap();
        let facade = Facade::new(File::open("/").unwrap(), b"/", b"/").unwrap();
        let mut status = AndroidStat::default();
        assert_eq!(
            unsafe { facade.fstat(external.as_raw_fd(), &mut status) },
            0
        );
        assert_eq!(status.st_size, 65_536);
        assert_eq!(status.st_mode & ANDROID_S_IFREG, ANDROID_S_IFREG);
        assert_eq!(darwin_art_bionic_fs_bind_host_descriptor_resolver(None), 0);
        drop(external);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn exact_dev_binder_open_uses_single_process_device_hook() {
        let _guard = PROCESS_TEST_LOCK.lock().unwrap();
        assert_eq!(
            darwin_art_bionic_fs_bind_binder_device_open(Some(open_binder)),
            0
        );
        assert_eq!(
            darwin_art_bionic_fs_bind_binder_device_open(Some(open_binder)),
            -1
        );
        assert_eq!(
            unsafe {
                darwin_art_bionic_fs_open_core(
                    c"/dev/binder".as_ptr(),
                    O_RDWR | O_CLOEXEC,
                    0,
                )
            },
            0x4000_0123
        );
        assert_eq!(darwin_art_bionic_fs_bind_binder_device_open(None), 0);
    }

    #[test]
    fn proc_executable_readlink_has_android_identity_and_linux_truncation() {
        let facade = Facade::new(File::open("/").unwrap(), b"/system", b"/system").unwrap();
        let mut complete = [0_u8; 64];
        assert_eq!(
            facade.readlink(
                b"/proc/self/exe",
                complete.as_mut_ptr().cast(),
                complete.len(),
            ),
            25
        );
        assert_eq!(&complete[..25], b"/system/bin/app_process64");
        assert_eq!(complete[25], 0);

        let mut truncated = [0_u8; 7];
        assert_eq!(
            facade.readlink(
                b"/proc/thread-self/exe",
                truncated.as_mut_ptr().cast(),
                truncated.len(),
            ),
            7
        );
        assert_eq!(&truncated, b"/system");
        assert_eq!(facade.readlink(b"/proc/self/exe", ptr::null_mut(), 0), -1);
    }

    #[test]
    fn proc_self_realpath_resolves_to_numeric_process_directory() {
        let facade = Facade::new(File::open("/").unwrap(), b"/system", b"/system").unwrap();
        let mut resolved = [0_i8; 4096];
        let result = unsafe { facade.realpath(b"/proc/self", resolved.as_mut_ptr()) };
        assert_eq!(result, resolved.as_mut_ptr());
        let actual = unsafe { CStr::from_ptr(resolved.as_ptr()) };
        assert_eq!(
            actual.to_bytes(),
            format!("/proc/{}", std::process::id()).as_bytes()
        );
    }

    #[test]
    fn descriptor_allocator_reserves_central_broker_token_range() {
        let mut table = DescriptorTable {
            next: CENTRAL_BROKER_TOKEN_MARKER - 1,
            ..DescriptorTable::default()
        };
        let boundary = table
            .insert(Descriptor::Random(RandomDeviceKind::Random))
            .unwrap();
        let wrapped = table
            .insert(Descriptor::Random(RandomDeviceKind::Urandom))
            .unwrap();
        assert_eq!(boundary, CENTRAL_BROKER_TOKEN_MARKER - 1);
        assert_eq!(wrapped, 10_000);
        assert_eq!(boundary & CENTRAL_BROKER_TOKEN_MARKER, 0);
        assert_eq!(wrapped & CENTRAL_BROKER_TOKEN_MARKER, 0);
    }

    #[test]
    fn descriptor_take_restore_preserves_flags_and_reuse_clears_them() {
        let mut table = DescriptorTable::default();
        let fd = table
            .insert_with_flags(Descriptor::Random(RandomDeviceKind::Random), true)
            .unwrap();
        let descriptor = table.take(fd).unwrap();
        assert_eq!(table.fd_flags(fd), Some(FD_CLOEXEC));
        table.next = fd; // Simulate allocation wrapping to the leased slot.
        let other = table.insert(Descriptor::Random(RandomDeviceKind::Urandom)).unwrap();
        assert_ne!(other, fd);
        table.restore(fd, descriptor);
        assert_eq!(table.fd_flags(fd), Some(FD_CLOEXEC));

        let descriptor = table.close_entry(fd).unwrap();
        assert_eq!(table.fd_flags(fd), None);
        drop(descriptor);
        let reused = table
            .insert_with_flags(Descriptor::Random(RandomDeviceKind::Urandom), false)
            .unwrap();
        assert_eq!(reused, fd);
        assert_eq!(table.fd_flags(reused), Some(0));
    }

    #[test]
    fn unknown_darwin_errno_publishes_eio_and_marks_capability_failure() {
        let facade = Facade::new(File::open("/").unwrap(), b"/system", b"/system").unwrap();
        Facade::set_android_errno(2);
        // SAFETY: Darwin returns the current pthread's host errno cell.
        unsafe { *__error() = 33_002 };

        assert_eq!(
            facade.fail_io(&std::io::Error::from_raw_os_error(123_456)),
            -1
        );
        assert!(facade.has_capability_failure());
        // SAFETY: the standalone errno provider is linked for this test binary.
        assert_eq!(unsafe { darwin_art_bionic_errno_load() }, ANDROID_EIO);
        // SAFETY: same pthread-local host errno cell set above.
        assert_eq!(unsafe { *__error() }, 33_002);
    }

    #[test]
    fn repeated_opendir_closedir_reclaims_side_table_records() {
        let facade = Facade::new(File::open("/").unwrap(), b"/system", b"/system").unwrap();
        for _ in 0..512 {
            let directory = facade.opendir(b"/system");
            assert!(!directory.is_null());
            assert_eq!(facade.closedir(directory), 0);
        }
        assert!(facade.directories.lock().unwrap().is_empty());
        assert!(facade.descriptors.lock().unwrap().entries.is_empty());
    }

    #[test]
    fn opendir_initializes_close_on_exec_and_fdopendir_preserves_it() {
        let facade = Facade::new(File::open("/").unwrap(), b"/system", b"/system").unwrap();
        let directory = facade.opendir(b"/system");
        assert!(!directory.is_null());
        let opendir_fd = facade.dirfd(directory);
        assert_eq!(facade.fcntl(opendir_fd, 1, 0), FD_CLOEXEC);
        assert_eq!(facade.closedir(directory), 0);

        let plain_fd = facade.open(b"/system", O_RDONLY | O_DIRECTORY);
        assert!(plain_fd >= 10_000);
        assert_eq!(facade.fcntl(plain_fd, 1, 0), 0);
        let plain_directory = facade.fdopendir(plain_fd);
        assert!(!plain_directory.is_null());
        // AOSP fdopendir adopts the supplied descriptor and does not alter
        // its descriptor flags.
        assert_eq!(facade.fcntl(plain_fd, 1, 0), 0);
        assert_eq!(facade.closedir(plain_directory), 0);

        let cloexec_fd = facade.open(b"/system", O_RDONLY | O_DIRECTORY | O_CLOEXEC);
        assert!(cloexec_fd >= 10_000);
        let cloexec_directory = facade.fdopendir(cloexec_fd);
        assert!(!cloexec_directory.is_null());
        assert_eq!(facade.fcntl(cloexec_fd, 1, 0), FD_CLOEXEC);
        assert_eq!(facade.closedir(cloexec_directory), 0);
    }

    #[test]
    fn directory_fd_preserves_guest_identity_until_closedir() {
        let facade = Facade::new(File::open("/").unwrap(), b"/system", b"/system").unwrap();
        let fd = facade.open(b"/system", 0);
        assert!(fd >= 0);
        let directory = facade.fdopendir(fd);
        assert!(!directory.is_null());
        assert_eq!(facade.dirfd(directory), fd);
        assert!(facade.descriptors.lock().unwrap().entries.contains_key(&fd));
        assert!(!facade.readdir(directory).is_null());
        assert_eq!(facade.closedir(directory), 0);
        assert!(!facade.descriptors.lock().unwrap().entries.contains_key(&fd));
        assert_eq!(facade.dirfd(directory), -1);
        assert_eq!(unsafe { darwin_art_bionic_errno_load() }, ANDROID_EBADF);
    }

    #[test]
    fn failed_fdopendir_preserves_callers_descriptor() {
        let facade = Facade::new(File::open("/").unwrap(), b"/system", b"/system").unwrap();
        let fd = facade.descriptors.lock().unwrap()
            .insert(Descriptor::File(File::open("/dev/null").unwrap())).unwrap();
        assert!(facade.fdopendir(fd).is_null());
        assert_eq!(unsafe { darwin_art_bionic_errno_load() }, ANDROID_ENOTDIR);
        assert!(facade.descriptors.lock().unwrap().entries.contains_key(&fd));
        assert_eq!(facade.close(fd), 0);
    }

    struct BlockingEntropy {
        state: Mutex<(bool, bool)>,
        condition: Condvar,
    }

    impl BlockingEntropy {
        fn new() -> Self {
            Self {
                state: Mutex::new((false, false)),
                condition: Condvar::new(),
            }
        }

        fn wait_until_entered(&self) {
            let state = self.state.lock().unwrap();
            drop(
                self.condition
                    .wait_while(state, |(entered, _)| !*entered)
                    .unwrap(),
            );
        }

        fn release(&self) {
            let mut state = self.state.lock().unwrap();
            state.1 = true;
            self.condition.notify_all();
        }
    }

    impl EntropyBackend for BlockingEntropy {
        fn fill(&self, bytes: &mut [u8]) -> Result<(), ()> {
            let mut state = self.state.lock().unwrap();
            state.0 = true;
            self.condition.notify_all();
            state = self
                .condition
                .wait_while(state, |(_, release)| !*release)
                .unwrap();
            drop(state);
            bytes.fill(0xa5);
            Ok(())
        }
    }

    #[test]
    fn random_read_serializes_close_and_reuses_descriptor_without_stale_kind() {
        let entropy = Arc::new(BlockingEntropy::new());
        let facade = Arc::new(
            Facade::new_with_entropy(
                File::open("/").unwrap(),
                b"/system",
                b"/system",
                entropy.clone(),
            )
            .unwrap(),
        );
        let fd = facade.open(b"/dev/random", O_RDONLY);
        assert!(fd >= 10_000);

        let reader_facade = facade.clone();
        let reader = thread::spawn(move || {
            let mut bytes = [0_u8; 32];
            // SAFETY: the local output remains writable for the whole call.
            let result = unsafe { reader_facade.read(fd, bytes.as_mut_ptr().cast(), bytes.len()) };
            (result, bytes)
        });
        entropy.wait_until_entered();

        let close_done = Arc::new(AtomicBool::new(false));
        let close_facade = facade.clone();
        let close_done_thread = close_done.clone();
        let closer = thread::spawn(move || {
            let result = close_facade.close(fd);
            close_done_thread.store(true, Ordering::Release);
            result
        });
        thread::sleep(Duration::from_millis(20));
        assert!(!close_done.load(Ordering::Acquire));
        entropy.release();
        let (read, bytes) = reader.join().unwrap();
        assert_eq!(read, 32);
        assert_eq!(bytes, [0xa5; 32]);
        assert_eq!(closer.join().unwrap(), 0);

        let reused = facade.open(b"/dev/urandom", O_RDONLY);
        assert_eq!(reused, fd);
        assert_eq!(facade.close(reused), 0);
    }

    #[test]
    fn random_devices_accept_android_descriptor_flags() {
        let facade = Facade::new(File::open("/").unwrap(), b"/system", b"/system").unwrap();
        let fd = facade.open(b"/dev/urandom", O_RDONLY | O_CLOEXEC | O_NONBLOCK);
        assert!(fd >= 10_000);
        assert_eq!(facade.close(fd), 0);
    }

    #[test]
    fn virtual_regular_file_readv_uses_one_vector_operation() {
        let project = File::open(concat!(env!("CARGO_MANIFEST_DIR"), "/../..")).unwrap();
        let facade = Facade::new(project, b"/", b"/").unwrap();
        let fd = facade.open(b"/Cargo.toml", O_RDONLY);
        assert!(fd >= 10_000);
        let mut first = [0_u8; 5];
        let mut second = [0_u8; 5];
        let vectors = [
            NativeIovec {
                base: first.as_mut_ptr().cast(),
                length: first.len(),
            },
            NativeIovec {
                base: second.as_mut_ptr().cast(),
                length: second.len(),
            },
        ];
        // SAFETY: both buffers and the vector array remain live for the call.
        assert_eq!(unsafe { facade.readv(fd, vectors.as_ptr(), 2) }, 10);
        assert_eq!(&first, b"[work");
        assert_eq!(&second, b"space");
        assert_eq!(facade.close(fd), 0);
    }

    #[test]
    fn private_root_open_rejects_final_symlink_without_canonicalize_race() {
        let private_root = std::env::temp_dir().join(format!(
            "darwin-art-private-nofollow-{}-{:?}",
            std::process::id(),
            thread::current().id()
        ));
        fs::create_dir_all(&private_root).unwrap();
        let outside = private_root.with_extension("-outside");
        fs::write(&outside, b"outside").unwrap();
        std::os::unix::fs::symlink(&outside, private_root.join("escape")).unwrap();
        let mut facade = Facade::new(File::open("/").unwrap(), b"/", b"/").unwrap();
        facade.private_root = Some(private_data::PrivateDataRoot::open(private_root.clone()).unwrap());
        assert_eq!(facade.open(private_root.join("escape").as_os_str().as_bytes(), O_RDONLY), -1);
        fs::remove_dir_all(private_root).unwrap();
        fs::remove_file(outside).unwrap();
    }

    #[test]
    fn private_file_fcntl_translates_android_descriptor_and_status_flags() {
        let private_root = std::env::temp_dir().join(format!(
            "darwin-art-fcntl-{}-{:?}",
            std::process::id(),
            thread::current().id()
        ));
        fs::create_dir_all(&private_root).unwrap();
        let mut facade = Facade::new(File::open("/").unwrap(), b"/", b"/").unwrap();
        facade.private_root = Some(private_data::PrivateDataRoot::open(private_root.clone()).unwrap());
        assert_eq!(facade.mkdir(b"/data/fcntl", 0o755), 0);
        let fd = facade.open(b"/data/fcntl/file", O_RDWR | O_CREAT | O_TRUNC);
        assert!(fd >= 10_000);

        assert_eq!(facade.fcntl(fd, 1, 0), 0);
        assert_eq!(facade.fcntl(fd, 2, 1), 0);
        assert_eq!(facade.fcntl(fd, 1, 0), 1);
        assert_eq!(facade.fcntl(fd, 2, 0), 0);
        assert_eq!(facade.fcntl(fd, 1, 0), 0);
        assert_eq!(facade.fcntl(fd, 3, 0) & O_ACCMODE, O_RDWR);
        assert_eq!(facade.fcntl(fd, 4, (O_APPEND | O_NONBLOCK) as isize), 0);
        let status = facade.fcntl(fd, 3, 0);
        assert_eq!(status & O_ACCMODE, O_RDWR);
        assert_ne!(status & O_APPEND, 0);
        assert_ne!(status & O_NONBLOCK, 0);
        assert_eq!(facade.fcntl(fd, 4, O_CREAT as isize), -1);
        assert_eq!(facade.close(fd), 0);
        fs::remove_dir_all(private_root).unwrap();
    }

    #[test]
    fn descriptor_flags_follow_android_dup_and_reuse_rules() {
        let facade = Facade::new(File::open("/").unwrap(), b"/system", b"/system").unwrap();
        let original = facade.open(b"/dev/null", O_RDONLY | O_CLOEXEC);
        assert!(original >= 10_000);
        assert_eq!(facade.fcntl(original, 1, 0), FD_CLOEXEC);

        let plain = facade.fcntl(original, 0, 0);
        assert!(plain >= 10_000);
        assert_eq!(facade.fcntl(plain, 1, 0), 0);

        let cloexec = facade.fcntl(original, 1030, 0);
        assert!(cloexec >= 10_000);
        assert_eq!(facade.fcntl(cloexec, 1, 0), FD_CLOEXEC);

        assert_eq!(facade.close(cloexec), 0);
        let reused = facade.open(b"/dev/urandom", O_RDONLY);
        assert_eq!(reused, cloexec);
        assert_eq!(facade.fcntl(reused, 1, 0), 0);

        assert_eq!(facade.close(reused), 0);
        assert_eq!(facade.close(plain), 0);
        assert_eq!(facade.close(original), 0);
    }

    #[test]
    fn absolute_private_host_read_and_android_create_use_host_capability() {
        let private_root = std::env::temp_dir().join(format!(
            "darwin-art-private-open-{}-{:?}",
            std::process::id(),
            thread::current().id()
        ));
        fs::create_dir_all(&private_root).unwrap();
        let existing = private_root.join("source.so");
        fs::write(&existing, b"ELF").unwrap();
        let missing = private_root.join("copy.so");

        let mut facade = Facade::new(File::open("/").unwrap(), b"/", b"/").unwrap();
        facade.private_root = Some(private_data::PrivateDataRoot::open(private_root.clone()).unwrap());

        let input_fd = facade.open(existing.as_os_str().as_bytes(), O_RDONLY);
        assert!(input_fd >= 10_000);
        let mut bytes = [0_u8; 3];
        assert_eq!(
            unsafe { facade.read(input_fd, bytes.as_mut_ptr().cast(), bytes.len()) },
            3
        );
        assert_eq!(&bytes, b"ELF");
        assert_eq!(facade.close(input_fd), 0);

        // 0xc1 is Android O_WRONLY|O_CREAT|O_EXCL. This must create the
        // missing final component below the private host root.
        let output_fd = facade.open(missing.as_os_str().as_bytes(), O_WRONLY | O_CREAT | O_EXCL);
        assert!(output_fd >= 10_000);
        assert_eq!(facade.close(output_fd), 0);
        assert!(missing.is_file());

        fs::remove_dir_all(private_root).unwrap();
    }

    #[test]
    fn proc_maps_accepts_self_thread_self_and_virtual_pid_aliases() {
        assert!(Facade::proc_self_maps(b"/proc/self/maps"));
        assert!(Facade::proc_self_maps(b"/proc/thread-self/maps"));
        let current = format!("/proc/{}/maps", std::process::id());
        assert!(Facade::proc_self_maps(current.as_bytes()));
        assert!(Facade::proc_self_maps(b"/proc/999999999/maps"));
        assert!(!Facade::proc_self_maps(b"/proc/0/maps"));
        assert!(!Facade::proc_self_maps(b"/proc/not-a-pid/maps"));
        assert!(!Facade::proc_self_maps(b"/proc/self/status"));
    }

    #[test]
    fn proc_smaps_is_a_readonly_android_memory_snapshot() {
        assert!(Facade::proc_self_smaps(b"/proc/self/smaps"));
        assert!(Facade::proc_self_smaps(b"/proc/thread-self/smaps"));
        let current = format!("/proc/{}/smaps", std::process::id());
        assert!(Facade::proc_self_smaps(current.as_bytes()));
        assert!(Facade::proc_self_smaps(b"/proc/999999999/smaps"));
        assert!(!Facade::proc_self_smaps(b"/proc/0/smaps"));
        assert!(!Facade::proc_self_smaps(b"/proc/self/maps"));
        assert!(!Facade::proc_self_smaps(b"/proc/not-a-pid/smaps"));

        let facade = Facade::new(File::open("/").unwrap(), b"/system", b"/system").unwrap();
        let fd = facade.open(b"/proc/self/smaps", O_RDONLY);
        assert!(fd >= 10_000);
        let mut contents = Vec::new();
        let mut chunk = [0_u8; 4096];
        loop {
            // SAFETY: chunk remains writable for the synchronous facade read.
            let count = unsafe { facade.read(fd, chunk.as_mut_ptr().cast(), chunk.len()) };
            assert!(count >= 0);
            if count == 0 {
                break;
            }
            contents.extend_from_slice(&chunk[..usize::try_from(count).unwrap()]);
        }
        let contents = String::from_utf8(contents).unwrap();
        assert!(contents.contains("Shared_Dirty:"));
        assert!(contents.contains("Private_Dirty:"));
        assert!(contents.contains("VmFlags:"));
        assert_eq!(facade.close(fd), 0);
    }

    #[test]
    fn regular_file_host_duplicate_is_owned_and_independent() {
        let project = File::open(concat!(env!("CARGO_MANIFEST_DIR"), "/../..")).unwrap();
        let facade = Arc::new(Facade::new(project, b"/", b"/").unwrap());
        let _activation = facade.activate();
        let fd = facade.open(b"/Cargo.toml", O_RDONLY);
        assert!(fd >= 10_000);

        let mut host_fd = -1;
        // SAFETY: host_fd is writable through this synchronous call.
        assert_eq!(
            unsafe { darwin_art_bionic_fs_dup_host_fd_core(fd, &mut host_fd) },
            1
        );
        assert!(host_fd >= 0);
        // SAFETY: a status of 1 transfers ownership of the returned raw fd.
        let mut duplicate = unsafe { File::from_raw_fd(host_fd) };
        let mut bytes = [0_u8; 5];
        duplicate.read_exact(&mut bytes).unwrap();
        assert_eq!(&bytes, b"[work");
        drop(duplicate);

        let mut original = [0_u8; 5];
        // SAFETY: original is writable for its complete length.
        assert_eq!(
            unsafe { facade.read(fd, original.as_mut_ptr().cast(), original.len()) },
            5
        );
        // POSIX dup shares the open-file description and therefore its offset;
        // the guest descriptor nevertheless remains live after closing the
        // duplicate.
        assert_eq!(&original, b"space");
        assert_eq!(facade.close(fd), 0);
    }

    #[test]
    fn ioctl_kind_lookup_is_atomic_with_close() {
        let facade =
            Arc::new(Facade::new(File::open("/").unwrap(), b"/system", b"/system").unwrap());
        let fd = facade.open(b"/dev/random", O_RDONLY);
        let barrier = Arc::new(Barrier::new(9));
        let mut workers = Vec::new();
        for _ in 0..8 {
            let worker_facade = facade.clone();
            let worker_barrier = barrier.clone();
            workers.push(thread::spawn(move || {
                let _activation = worker_facade.activate();
                worker_barrier.wait();
                for _ in 0..2_000 {
                    let mut info = IoctlFdInfo::default();
                    // SAFETY: info remains writable through this synchronous callback.
                    let status = unsafe {
                        darwin_art_bionic_fs_ioctl_fd_lookup(ptr::null_mut(), fd, &mut info)
                    };
                    assert!(status == IOCTL_FD_FOUND || status == IOCTL_FD_BAD);
                    if status == IOCTL_FD_FOUND {
                        assert_eq!(info.kind, IOCTL_FD_RANDOM_DEVICE);
                    }
                }
            }));
        }
        barrier.wait();
        assert_eq!(facade.close(fd), 0);
        for worker in workers {
            worker.join().unwrap();
        }
    }

    #[test]
    fn private_data_overlay_persists_and_sendfile_preserves_offset_rules() {
        let project = File::open(concat!(env!("CARGO_MANIFEST_DIR"), "/../..")).unwrap();
        let facade = Facade::new(project, b"/", b"/").unwrap();
        assert_eq!(facade.mkdir(b"/data/cache", 0o755), 0);
        let input = facade.open(b"/Cargo.toml", O_RDONLY);
        let output = facade.open(b"/data/cache/copy", O_WRONLY | O_CREAT | O_TRUNC);
        assert!(input >= 10_000 && output >= 10_000);

        let mut result = SendfileResult::default();
        let request = SendfileRequest {
            abi_version: SENDFILE_ABI_VERSION,
            output_fd: output,
            input_fd: input,
            has_explicit_offset: 0,
            offset: 0,
            count: 5,
        };
        assert_eq!(
            facade.sendfile_transfer(&request, &mut result),
            SENDFILE_TRANSFER_OK
        );
        assert_eq!(result.transferred, 5);
        assert_eq!(facade.close(output), 0);

        let persisted = facade.open(b"/data/cache/copy", O_RDONLY);
        let mut first = [0_u8; 5];
        assert_eq!(
            unsafe { facade.read(persisted, first.as_mut_ptr().cast(), 5) },
            5
        );
        assert_eq!(&first, b"[work");
        assert_eq!(facade.close(persisted), 0);
        assert_eq!(facade.chmod(b"/data/cache/copy", 0o640), 0);
        let mut chmod_status = AndroidStat::default();
        assert_eq!(
            unsafe { facade.stat(b"/data/cache/copy", &mut chmod_status, false) },
            0
        );
        assert_eq!(chmod_status.st_mode & 0o7777, 0o640);

        let offset_output = facade.open(b"/data/offset", O_WRONLY | O_CREAT | O_TRUNC);
        let explicit = SendfileRequest {
            output_fd: offset_output,
            has_explicit_offset: 1,
            offset: 1,
            count: 3,
            ..request
        };
        assert_eq!(
            facade.sendfile_transfer(&explicit, &mut result),
            SENDFILE_TRANSFER_OK
        );
        assert_eq!((result.transferred, result.next_offset), (3, 4));
        let current = SendfileRequest {
            output_fd: offset_output,
            count: 1,
            ..request
        };
        assert_eq!(
            facade.sendfile_transfer(&current, &mut result),
            SENDFILE_TRANSFER_OK
        );
        assert_eq!(result.transferred, 1);
        assert_eq!(facade.close(offset_output), 0);
        let offset_copy = facade.open(b"/data/offset", O_RDONLY);
        let mut bytes = [0_u8; 4];
        assert_eq!(
            unsafe { facade.read(offset_copy, bytes.as_mut_ptr().cast(), 4) },
            4
        );
        assert_eq!(&bytes, b"wors");
        assert_eq!(facade.close(offset_copy), 0);

        let positioned = facade.open(b"/data/positioned", O_WRONLY | O_CREAT | O_TRUNC);
        assert_eq!(
            unsafe { facade.pwrite(positioned, b"chrome".as_ptr().cast(), 6, 3) },
            6
        );
        assert_eq!(facade.lseek(positioned, 0, 1), 0);
        assert_eq!(facade.close(positioned), 0);
        let positioned = facade.open(b"/data/positioned", O_RDONLY);
        let mut positioned_bytes = [0xff_u8; 9];
        assert_eq!(
            unsafe { facade.read(positioned, positioned_bytes.as_mut_ptr().cast(), 9) },
            9
        );
        assert_eq!(&positioned_bytes, b"\0\0\0chrome");
        assert_eq!(facade.close(positioned), 0);

        let truncate = facade.open(b"/data/cache/copy", O_WRONLY | O_TRUNC);
        assert_eq!(facade.ftruncate(truncate, 2), 0);
        let mut status = AndroidStat::default();
        assert_eq!(unsafe { facade.fstat(truncate, &mut status) }, 0);
        assert_eq!(status.st_size, 2);
        assert_eq!(facade.close(truncate), 0);
        assert_eq!(facade.open(b"/Cargo.toml", O_WRONLY | O_TRUNC), -1);
    }

    #[test]
    fn mkdirat_retains_dirfd_cwd_and_mount_authority() {
        // This fixture supplies a process environment mount. Isolate it from
        // parallel tests constructing their own Facades in the parent process.
        const CHILD: &str = "DARWIN_ART_MKDIRAT_TEST_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "tests::mkdirat_retains_dirfd_cwd_and_mount_authority"])
                .env(CHILD, "1")
                .status()
                .unwrap();
            assert!(status.success());
            return;
        }
        let _serial = PROCESS_TEST_LOCK.lock().unwrap();
        let base = std::env::temp_dir().join(format!(
            "darwin-art-mkdirat-{}-{:?}",
            std::process::id(),
            thread::current().id()
        ));
        fs::create_dir_all(base.join("root/system")).unwrap();
        fs::create_dir_all(base.join("data/work")).unwrap();
        fs::write(base.join("data/work/file"), b"file").unwrap();

        let previous = std::env::var_os("DARWIN_ART_ANDROID_PRIVATE_DATA_ROOT");
        // SAFETY: this test serializes the process-wide environment mutation.
        unsafe { std::env::set_var("DARWIN_ART_ANDROID_PRIVATE_DATA_ROOT", base.join("data")) };
        let facade = Facade::new(File::open(base.join("root")).unwrap(), b"/", b"/").unwrap();
        // SAFETY: restore the caller's process environment before exercising the facade.
        unsafe {
            match previous {
                Some(value) => std::env::set_var("DARWIN_ART_ANDROID_PRIVATE_DATA_ROOT", value),
                None => std::env::remove_var("DARWIN_ART_ANDROID_PRIVATE_DATA_ROOT"),
            }
        }

        let directory_fd = facade.open(b"/data/work", O_RDONLY | O_DIRECTORY);
        assert!(directory_fd >= 10_000);
        fs::rename(base.join("data"), base.join("retained-data")).unwrap();
        fs::create_dir(base.join("data")).unwrap();
        assert_eq!(facade.mkdir_at(directory_fd, b"dirfd-child", 0o751), 0);
        assert!(base.join("retained-data/work/dirfd-child").is_dir());
        assert!(!base.join("data/work/dirfd-child").exists());

        assert_eq!(facade.mkdir_at(directory_fd, b".", 0o700), -1);
        assert_eq!(unsafe { darwin_art_bionic_errno_load() }, ANDROID_EEXIST);
        assert_eq!(facade.mkdir_at(directory_fd, b"..", 0o700), -1);
        assert_eq!(unsafe { darwin_art_bionic_errno_load() }, ANDROID_EEXIST);
        assert_eq!(facade.mkdir_at(directory_fd, b"", 0o700), -1);
        assert_eq!(unsafe { darwin_art_bionic_errno_load() }, ANDROID_ENOENT);
        assert_eq!(facade.mkdir_at(-7, b"missing", 0o700), -1);
        assert_eq!(unsafe { darwin_art_bionic_errno_load() }, ANDROID_EBADF);
        assert_eq!(facade.mkdir_at(-7, b"/data/absolute", 0o750), 0);
        assert!(base.join("retained-data/absolute").is_dir());

        let file_fd = facade.open(b"/data/work/file", O_RDONLY);
        assert!(file_fd >= 10_000);
        assert_eq!(facade.mkdir_at(file_fd, b"child", 0o700), -1);
        assert_eq!(unsafe { darwin_art_bionic_errno_load() }, ANDROID_ENOTDIR);
        assert_eq!(facade.close(file_fd), 0);

        assert_eq!(facade.chdir(b"/data/work"), 0);
        fs::rename(base.join("retained-data/work"), base.join("retained-data/moved-work"))
            .unwrap();
        fs::create_dir(base.join("retained-data/work")).unwrap();
        assert_eq!(facade.mkdir_at(AT_FDCWD, b"cwd-child", 0o700), 0);
        assert!(base.join("retained-data/moved-work/cwd-child").is_dir());

        let system_fd = facade.open(b"/system", O_RDONLY | O_DIRECTORY);
        assert!(system_fd >= 10_000);
        assert_eq!(facade.mkdir_at(system_fd, b"new", 0o700), -1);
        assert_eq!(unsafe { darwin_art_bionic_errno_load() }, ANDROID_EROFS);
        assert_eq!(facade.close(system_fd), 0);
        assert_eq!(facade.mkdir_at(directory_fd, b"../../system/escape", 0o700), -1);
        assert_eq!(unsafe { darwin_art_bionic_errno_load() }, ANDROID_EROFS);
        assert_eq!(facade.close(directory_fd), 0);

        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn fchown_does_not_acknowledge_unstored_overlay_ownership() {
        let facade = Facade::new(File::open("/").unwrap(), b"/", b"/").unwrap();
        let node = Arc::new(Mutex::new(OverlayFile {
            inode: 99,
            mode: ANDROID_S_IFREG | 0o600,
            data: b"unchanged".to_vec(),
        }));
        let fd = facade.descriptors.lock().unwrap().insert_with_flags(
            Descriptor::Overlay(OverlayDescriptor {
                node: node.clone(), offset: 0, readable: true, writable: true,
            }),
            false,
        ).unwrap();
        assert_eq!(facade.fchown(fd, 12345, 23456), -1);
        assert_eq!(unsafe { darwin_art_bionic_errno_load() }, ANDROID_EOPNOTSUPP);
        assert_eq!(node.lock().unwrap().data, b"unchanged");
        assert_eq!(node.lock().unwrap().mode, ANDROID_S_IFREG | 0o600);
        assert_eq!(facade.close(fd), 0);
        assert_eq!(facade.fchown(fd, 12345, 23456), -1);
        assert_eq!(unsafe { darwin_art_bionic_errno_load() }, ANDROID_EBADF);
    }

    #[test]
    fn private_data_seed_creates_only_authorized_overlay_hierarchy() {
        let facade = Facade::new(File::open("/").unwrap(), b"/", b"/").unwrap();
        assert_eq!(
            facade.seed_private_directory(b"/data/user/0/org.chromium.chrome"),
            0
        );
        assert_eq!(
            facade.mkdir(b"/data/user/0/org.chromium.chrome/files", 0o700),
            0
        );
        assert_eq!(
            facade.mkdir(b"/data/user/0/org.chromium.chrome/files/splitcompat", 0o700),
            0
        );
        assert_eq!(facade.seed_private_directory(b"/system/not-private"), -1);
    }

    #[test]
    fn system_data_atomic_rename_persists_and_is_not_in_app_namespace() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!(
            "darwin-art-system-data-{}-{:?}", std::process::id(), thread::current().id()
        ));
        let system_root = root.join("system/private-data");
        let app_root = root.join("app/private-data");
        fs::create_dir_all(&app_root).unwrap();
        let helper = concat!(env!("CARGO_MANIFEST_DIR"), "/../lib/system-private-data.sh");
        let provision = || std::process::Command::new("bash")
            .args(["-c", "source \"$1\"; prepare_system_private_data \"$2\"", "test"])
            .arg(helper).arg(&system_root).status().unwrap();
        assert!(provision().success());
        assert_eq!(fs::metadata(&system_root).unwrap().permissions().mode() & 0o777, 0o500);
        let mut system = Facade::new(File::open("/").unwrap(), b"/", b"/").unwrap();
        system.private_root = Some(private_data::PrivateDataRoot::open(system_root.clone()).unwrap());
        let temporary = b"/data/system/package-dex-usage.list.new";
        let final_path = b"/data/system/package-dex-usage.list";
        let fd = system.open(temporary, O_WRONLY | O_CREAT | O_TRUNC);
        assert!(fd >= 10_000);
        let bytes = b"PACKAGE_MANAGER__PACKAGE_DEX_USAGE__2\n";
        assert_eq!(unsafe { system.write(fd, bytes.as_ptr().cast(), bytes.len()) }, bytes.len() as isize);
        assert_eq!(system.fsync(fd), 0);
        assert_eq!(system.close(fd), 0);
        assert_eq!(system.rename_path(temporary, final_path), 0);
        assert!(provision().success());
        let mut reopened = Facade::new(File::open("/").unwrap(), b"/", b"/").unwrap();
        reopened.private_root = Some(private_data::PrivateDataRoot::open(system_root.clone()).unwrap());
        let fd = reopened.open(final_path, O_RDONLY);
        assert!(fd >= 10_000);
        let mut actual = vec![0; bytes.len()];
        assert_eq!(unsafe { reopened.read(fd, actual.as_mut_ptr().cast(), actual.len()) }, bytes.len() as isize);
        assert_eq!(actual, bytes);
        assert_eq!(reopened.close(fd), 0);
        let mut app = Facade::new(File::open("/").unwrap(), b"/", b"/").unwrap();
        app.private_root = Some(private_data::PrivateDataRoot::open(app_root).unwrap());
        assert_eq!(app.open(final_path, O_RDONLY), -1);
        // A preexisting symlink must not grant the service a different directory.
        fs::set_permissions(&system_root, fs::Permissions::from_mode(0o700)).unwrap();
        fs::rename(system_root.join("system"), root.join("saved-system")).unwrap();
        std::os::unix::fs::symlink(root.join("saved-system"), system_root.join("system")).unwrap();
        assert!(!provision().success());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn process_owner_cross_thread_rollback_duplicate_and_quiescent_uninstall() {
        let _serial = PROCESS_TEST_LOCK.lock().unwrap();
        assert_eq!(
            darwin_art_bionic_fs_process_uninstall(),
            PROCESS_OWNER_NOT_INSTALLED
        );
        // Missing root authority must not install a substitute filesystem.
        assert_eq!(
            unsafe {
                darwin_art_bionic_fs_process_install(
                    -1, b"/".as_ptr(), 1, b"/".as_ptr(), 1,
                )
            },
            PROCESS_OWNER_INVALID_ARGUMENT
        );
        assert_eq!(darwin_art_bionic_fs_process_uninstall(), PROCESS_OWNER_NOT_INSTALLED);
        let root = File::open("/").unwrap();
        // SAFETY: all byte slices and the borrowed directory fd remain live
        // through each synchronous installation attempt.
        assert_eq!(
            unsafe {
                darwin_art_bionic_fs_process_install(
                    root.as_raw_fd(),
                    b"/".as_ptr(),
                    1,
                    ptr::null(),
                    0,
                )
            },
            PROCESS_OWNER_INVALID_ARGUMENT
        );
        assert_eq!(
            unsafe {
                darwin_art_bionic_fs_process_install(
                    root.as_raw_fd(),
                    b"/".as_ptr(),
                    1,
                    b"/darwin-art-missing-process-owner-cwd".as_ptr(),
                    b"/darwin-art-missing-process-owner-cwd".len(),
                )
            },
            PROCESS_OWNER_CREATE_FAILED
        );
        assert_eq!(
            unsafe {
                darwin_art_bionic_fs_process_install(
                    root.as_raw_fd(),
                    b"/".as_ptr(),
                    1,
                    b"/".as_ptr(),
                    1,
                )
            },
            PROCESS_OWNER_OK
        );
        assert_eq!(
            unsafe {
                darwin_art_bionic_fs_process_install(
                    root.as_raw_fd(),
                    b"/".as_ptr(),
                    1,
                    b"/".as_ptr(),
                    1,
                )
            },
            PROCESS_OWNER_ALREADY_INSTALLED
        );
        let worker = thread::spawn(|| {
            // SAFETY: static paths and local output records remain live for
            // each call. This pthread has no TLS Activation guard.
            let fd =
                unsafe { darwin_art_bionic_fs_open_core(c"/dev/random".as_ptr(), O_RDONLY, 0) };
            assert!(fd >= 10_000);
            let mut bytes = [0_u8; 16];
            assert_eq!(
                unsafe {
                    darwin_art_bionic_fs_read_core(fd, bytes.as_mut_ptr().cast(), bytes.len())
                },
                bytes.len() as isize
            );
            let mut info = IoctlFdInfo::default();
            assert_eq!(
                unsafe { darwin_art_bionic_fs_ioctl_fd_lookup(ptr::null_mut(), fd, &mut info) },
                IOCTL_FD_FOUND
            );
            assert_eq!(info.kind, IOCTL_FD_RANDOM_DEVICE);
            assert_eq!(darwin_art_bionic_fs_close_core(fd), 0);
        });
        worker.join().unwrap();
        assert_eq!(darwin_art_bionic_fs_process_has_capability_failure(), 0);
        assert_eq!(darwin_art_bionic_fs_process_uninstall(), PROCESS_OWNER_OK);

        let entropy = Arc::new(BlockingEntropy::new());
        let facade = Arc::new(
            Facade::new_with_entropy(File::open("/").unwrap(), b"/", b"/", entropy.clone())
                .unwrap(),
        );
        assert_eq!(publish_process_facade(facade), PROCESS_OWNER_OK);
        // SAFETY: static path remains live for the call.
        let fd = unsafe { darwin_art_bionic_fs_open_core(c"/dev/random".as_ptr(), O_RDONLY, 0) };
        let reader = thread::spawn(move || {
            let mut bytes = [0_u8; 32];
            // SAFETY: local output remains writable through the call.
            unsafe { darwin_art_bionic_fs_read_core(fd, bytes.as_mut_ptr().cast(), bytes.len()) }
        });
        entropy.wait_until_entered();
        let uninstall_done = Arc::new(AtomicBool::new(false));
        let uninstall_done_thread = uninstall_done.clone();
        let uninstaller = thread::spawn(move || {
            let status = darwin_art_bionic_fs_process_uninstall();
            uninstall_done_thread.store(true, Ordering::Release);
            status
        });
        {
            let state = process_owner_state();
            drop(
                PROCESS_OWNER
                    .quiescent
                    .wait_while(state, |state| !state.draining)
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            );
        }
        assert!(!uninstall_done.load(Ordering::Acquire));
        assert_eq!(darwin_art_bionic_fs_process_uninstall(), PROCESS_OWNER_BUSY);
        // New calls fail closed once draining begins; they cannot extend the
        // lifetime being awaited by uninstall.
        assert_eq!(
            unsafe { darwin_art_bionic_fs_open_core(c"/dev/random".as_ptr(), O_RDONLY, 0) },
            -1
        );
        entropy.release();
        assert_eq!(reader.join().unwrap(), 32);
        assert_eq!(uninstaller.join().unwrap(), PROCESS_OWNER_OK);
        assert_eq!(darwin_art_bionic_fs_process_has_capability_failure(), -1);
        let mut info = IoctlFdInfo::default();
        assert_eq!(
            unsafe { darwin_art_bionic_fs_ioctl_fd_lookup(ptr::null_mut(), fd, &mut info) },
            IOCTL_FD_CAPABILITY_UNAVAILABLE
        );
        assert_eq!(
            darwin_art_bionic_fs_process_uninstall(),
            PROCESS_OWNER_NOT_INSTALLED
        );
    }

    #[test]
    fn synthetic_device_procfs_is_bounded_and_consistent() {
        // The topology shim is an internal host query and must not leak a
        // Darwin errno change into the calling guest operation.
        unsafe { *__error() = 33_210 };
        let (configured, online) = host_cpu_counts();
        assert_eq!(unsafe { *__error() }, 33_210);
        let cpuinfo = Facade::synthetic_proc_contents(b"/proc/cpuinfo").unwrap();
        let cpuinfo = String::from_utf8(cpuinfo).unwrap();
        assert_eq!(cpuinfo.matches("processor\t:").count(), online);
        let meminfo = String::from_utf8(
            Facade::synthetic_proc_contents(b"/proc/meminfo").unwrap(),
        )
        .unwrap();
        assert!(meminfo.contains("MemTotal:        8388608 kB"));
        // ProcfsMemoryUtil requires every one of these fields before it will
        // construct a snapshot. The device provider supplies one complete,
        // internally consistent Android view rather than leaking Darwin's
        // differently-defined process accounting fields.
        for path in [b"/proc/self/status".as_slice(), b"/proc/4242/status"] {
            let status = String::from_utf8(Facade::synthetic_proc_contents(path).unwrap()).unwrap();
            for field in ["Uid:", "VmHWM:", "VmRSS:", "RssAnon:", "RssShmem:", "VmSwap:"] {
                assert!(status.contains(field), "missing {field} from {status}");
            }
        }
        assert_eq!(
            Facade::synthetic_proc_contents(b"/proc/self/oom_score_adj"),
            Some(b"0\n".to_vec())
        );
        assert!(Facade::synthetic_proc_contents(b"/proc/self/cmdline")
            .is_some_and(|contents| contents.ends_with(&[0])));
        assert_eq!(
            Facade::synthetic_proc_contents(
                format!(
                    "/sys/devices/system/cpu/cpu{}/cpufreq/cpuinfo_max_freq",
                    configured - 1
                )
                .as_bytes(),
            ),
            Some(if configured - 1 < configured / 2 {
                b"1800000\n".to_vec()
            } else {
                b"2400000\n".to_vec()
            })
        );
        assert_eq!(
            Facade::synthetic_proc_contents(b"/sys/devices/system/cpu/cpu0/cpu_capacity"),
            Some(if configured / 2 > 0 {
                b"512\n".to_vec()
            } else {
                b"1024\n".to_vec()
            })
        );
        assert_eq!(
            Facade::synthetic_proc_contents(
                b"/sys/devices/system/cpu/cpu0/cpufreq/cpuinfo_max_freq"
            ),
            Some(if configured / 2 > 0 {
                b"1800000\n".to_vec()
            } else {
                b"2400000\n".to_vec()
            })
        );
        assert_eq!(
            Facade::synthetic_proc_contents(b"/sys/devices/system/cpu/possible"),
            Some(format!("0-{}\n", configured - 1).into_bytes())
        );
        assert_eq!(
            Facade::synthetic_proc_contents(b"/sys/devices/system/cpu/online"),
            Some(format!("0-{}\n", online - 1).into_bytes())
        );
        assert!(Facade::synthetic_proc_contents(
            format!(
                "/sys/devices/system/cpu/cpu{}/cpu_capacity",
                configured
            )
            .as_bytes()
        )
        .is_none());
        assert!(Facade::synthetic_proc_contents(
            format!(
                "/sys/devices/system/cpu/cpu{}/cpufreq/cpuinfo_max_freq",
                configured
            )
            .as_bytes()
        )
        .is_none());
    }

    #[test]
    fn ordinary_paths_bypass_synthetic_cpu_topology() {
        assert_eq!(
            Facade::synthetic_proc_contents(b"/system/etc/payload.txt"),
            None
        );
        assert_eq!(
            Facade::synthetic_proc_contents(b"/proc/not-a-synthetic-surface"),
            None
        );
    }
}
