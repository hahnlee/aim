use crate::System;
use aim_binder_host::{
    local::{Call, LocalProcess, Reply},
    parcel::{BAD_VALUE, FAILED_TRANSACTION, PERMISSION_DENIED, Parcel, UNKNOWN_TRANSACTION},
};
use std::{io::Write, sync::Arc};

pub const DUMP_TRANSACTION: u32 = u32::from_be_bytes(*b"_DMP");

fn user(args: &[String]) -> Result<i32, &'static str> {
    match args {
        [flag, user] if flag == "--role-package-state-hash" => user
            .parse::<i32>()
            .ok()
            .filter(|user| *user >= 0)
            .ok_or("role diagnostic user must be a nonnegative integer"),
        [] => Err("ordinary service-host dump is unsupported"),
        _ => Err("expected --role-package-state-hash USER"),
    }
}
pub fn run(process: &Arc<LocalProcess>, system: &Arc<System>, call: &mut Call<'_>) -> Reply {
    if call.sender_euid != 1000 {
        return Err(PERMISSION_DENIED);
    }
    let fd = call.data.read_fd()?;
    let mut out = process
        .file(fd)
        .and_then(|file| aim_binder_host::server::file_fd(&file))
        .ok_or(BAD_VALUE)?;
    // This diagnostic's public caller passes a fresh regular file directly to IBinder.dump.
    // Reject pipes/sockets before IPC; do not change the caller's shared OFD flags.
    if !out.metadata().map_err(|_| BAD_VALUE)?.is_file() {
        return Err(BAD_VALUE);
    }
    out.write(&[]).map_err(|_| BAD_VALUE)?;
    let args = aim_service_aidl::read_string_list(&mut call.data)?.ok_or(BAD_VALUE)?;
    let args = args
        .into_iter()
        .collect::<Option<Vec<_>>>()
        .ok_or(BAD_VALUE)?;
    if call.data.remaining() != 0 {
        return Err(BAD_VALUE);
    }
    let user = match user(&args) {
        Ok(user) => user,
        Err(message) => {
            writeln!(out, "role diagnostic argument error: {message}")
                .map_err(|_| FAILED_TRANSACTION)?;
            return Err(if args.is_empty() {
                UNKNOWN_TRANSACTION
            } else {
                BAD_VALUE
            });
        }
    };
    let result = (|| {
        // One retained scope checks the live bridge generation around BOTH calls.
        let diagnostic = system.role_hash_diagnostic()?;
        let first = diagnostic.compute(user)?;
        let second = diagnostic.compute(user)?;
        if first != second {
            return Err("original role hash changed between reads".to_owned());
        }
        Ok((diagnostic.bridge_handle(), first, second))
    })();
    match result {
        Ok((epoch, first, second)) => {
            writeln!(out, "ROLE_HASH\t{epoch}\t{user}\t{first}\t{second}")
                .map_err(|_| FAILED_TRANSACTION)?;
            let mut reply = Parcel::new();
            reply.write_no_exception();
            Ok(reply)
        }
        Err(error) => {
            writeln!(out, "role diagnostic owner error: {error}")
                .map_err(|_| FAILED_TRANSACTION)?;
            Err(FAILED_TRANSACTION)
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn denied_uid_is_rejected_before_fd_or_owner_access() {
        let driver = aim_binder_driver::Driver::new();
        let process = aim_binder_host::local::LocalProcess::open(
            &driver,
            aim_binder_driver::Device::Binder,
            aim_binder_driver::Credentials {
                pid: std::process::id() as i32,
                euid: 1000,
                security_context: None,
            },
        );
        let system = System::new(process.clone(), &[]);
        let parcel = Parcel::new();
        let mut call = Call {
            code: DUMP_TRANSACTION,
            flags: 0,
            sender_pid: 77,
            sender_euid: 2000,
            data: parcel.reader(),
        };
        assert_eq!(
            run(&process, &system, &mut call).unwrap_err(),
            PERMISSION_DENIED
        );
        assert!(
            system
                .role_hash_diagnostic()
                .err()
                .expect("missing owner was accepted")
                .contains("bridge absent")
        );
    }
    #[test]
    fn actual_fileport_trailing_data_and_owner_error_release_writers() {
        use aim_binder_host::local::Service;
        use std::os::fd::AsFd;
        struct Endpoint {
            process: Arc<LocalProcess>,
            system: Arc<System>,
        }
        impl Service for Endpoint {
            fn descriptor(&self) -> &str {
                "dev.aim.server.IServiceHost"
            }
            fn accepts_fds(&self) -> bool {
                true
            }
            fn transact(&self, call: &mut Call<'_>) -> Reply {
                run(&self.process, &self.system, call)
            }
        }
        let driver = aim_binder_driver::Driver::new();
        let process = LocalProcess::open(
            &driver,
            aim_binder_driver::Device::Binder,
            aim_binder_driver::Credentials {
                pid: std::process::id() as i32,
                euid: 1000,
                security_context: None,
            },
        );
        let system = System::new(process.clone(), &[]);
        let endpoint = process.add_service(Arc::new(Endpoint {
            process: process.clone(),
            system,
        }));
        let aim_binder_host::parcel::Binder::Local(id) = endpoint else {
            panic!("not local");
        };
        let endpoint = process.local_service(id).unwrap();
        let path =
            std::env::temp_dir().join(format!("aim-role-dump-fileport-{}", std::process::id()));
        let output = std::fs::OpenOptions::new()
            .write(true)
            .read(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        for trailing in [true, false] {
            let file = aim_binder_host::server::file_from_fd(output.as_fd())
                .expect("actual Mach fileport");
            let weak = Arc::downgrade(&file);
            let mut request = Parcel::new();
            request.write_file(file);
            request.write_i32(2);
            request.write_string16(Some("--role-package-state-hash"));
            request.write_string16(Some("0"));
            if trailing {
                request.write_i32(9);
            }
            let error = endpoint
                .transact(DUMP_TRANSACTION, &request, false)
                .err()
                .expect("request incorrectly succeeded");
            assert_eq!(
                error,
                if trailing {
                    BAD_VALUE
                } else {
                    FAILED_TRANSACTION
                }
            );
            drop(request);
            assert!(weak.upgrade().is_none(), "dump retained a writer fileport");
        }
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("system_server bridge absent"));
        assert!(!text.contains("ROLE_HASH"));
        drop(output);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn actual_unread_full_pipe_is_rejected_without_shared_flag_mutation() {
        use aim_binder_host::local::Service;
        use std::os::fd::{AsFd, AsRawFd, FromRawFd};
        struct Endpoint {
            process: Arc<LocalProcess>,
            system: Arc<System>,
        }
        impl Service for Endpoint {
            fn descriptor(&self) -> &str {
                "dev.aim.server.IServiceHost"
            }
            fn accepts_fds(&self) -> bool {
                true
            }
            fn transact(&self, call: &mut Call<'_>) -> Reply {
                run(&self.process, &self.system, call)
            }
        }
        let driver = aim_binder_driver::Driver::new();
        let process = LocalProcess::open(
            &driver,
            aim_binder_driver::Device::Binder,
            aim_binder_driver::Credentials {
                pid: std::process::id() as i32,
                euid: 1000,
                security_context: None,
            },
        );
        let endpoint = process.add_service(Arc::new(Endpoint {
            process: process.clone(),
            system: System::new(process.clone(), &[]),
        }));
        let aim_binder_host::parcel::Binder::Local(id) = endpoint else {
            panic!("not local");
        };
        let endpoint = process.local_service(id).unwrap();
        let mut fds = [-1; 2];
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        let read = unsafe { std::fs::File::from_raw_fd(fds[0]) };
        let write = unsafe { std::fs::File::from_raw_fd(fds[1]) };
        let flags = unsafe { libc::fcntl(write.as_raw_fd(), libc::F_GETFL) };
        assert_eq!(
            unsafe { libc::fcntl(write.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) },
            0
        );
        let bytes = [1u8; 4096];
        loop {
            let n = unsafe { libc::write(write.as_raw_fd(), bytes.as_ptr().cast(), bytes.len()) };
            if n < 0 {
                assert_eq!(
                    std::io::Error::last_os_error().kind(),
                    std::io::ErrorKind::WouldBlock
                );
                break;
            }
        }
        assert_eq!(
            unsafe { libc::fcntl(write.as_raw_fd(), libc::F_SETFL, flags) },
            0
        );
        let file = aim_binder_host::server::file_from_fd(write.as_fd()).unwrap();
        let weak = Arc::downgrade(&file);
        let flags = unsafe { libc::fcntl(write.as_raw_fd(), libc::F_GETFL) }; // transport export has already applied its own fileport contract
        let mut request = Parcel::new();
        request.write_file(file);
        request.write_i32(2);
        request.write_string16(Some("--role-package-state-hash"));
        request.write_string16(Some("0"));
        let start = std::time::Instant::now();
        assert_eq!(
            endpoint.transact(DUMP_TRANSACTION, &request, false).err(),
            Some(BAD_VALUE)
        );
        assert!(start.elapsed() < std::time::Duration::from_secs(1));
        assert_eq!(
            unsafe { libc::fcntl(write.as_raw_fd(), libc::F_GETFL) },
            flags
        );
        drop(request);
        assert!(weak.upgrade().is_none());
        drop(read);
        drop(write);
    }
    #[test]
    fn user_arguments_fail_closed() {
        assert_eq!(
            user(&["--role-package-state-hash".into(), "0".into()]),
            Ok(0)
        );
        for args in [
            vec![],
            vec!["--unknown".into(), "0".into()],
            vec!["--role-package-state-hash".into(), "-1".into()],
            vec!["--role-package-state-hash".into(), "2147483648".into()],
            vec![
                "--role-package-state-hash".into(),
                "0".into(),
                "extra".into(),
            ],
        ] {
            assert!(user(&args).is_err());
        }
    }
}
