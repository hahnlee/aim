//! macOS process responsibility for host device access.

use std::ffi::{CStr, CString, OsStr, OsString};
use std::io;
use std::os::fd::RawFd;
use std::os::unix::ffi::OsStrExt;

/// The process responsible for this process's macOS privacy requests.
pub fn responsible_pid() -> io::Result<libc::pid_t> {
    type Responsible = unsafe extern "C" fn(libc::pid_t) -> libc::pid_t;
    // SAFETY: libSystem is always loaded and the symbol has this C ABI.
    let symbol = unsafe {
        libc::dlsym(
            libc::RTLD_DEFAULT,
            c"responsibility_get_pid_responsible_for_pid".as_ptr(),
        )
    };
    if symbol.is_null() {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "libSystem lacks responsibility_get_pid_responsible_for_pid",
        ));
    }
    let function: Responsible = unsafe { std::mem::transmute(symbol) };
    let pid = unsafe { function(libc::getpid()) };
    if pid < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(pid)
    }
}

/// Disclaim responsibility for a child. A missing libSystem symbol is an error.
///
/// # Safety
/// `attr` must be an initialized, exclusively borrowed spawn attribute object.
pub unsafe fn disclaim(attr: &mut libc::posix_spawnattr_t) -> io::Result<()> {
    type Disclaim = unsafe extern "C" fn(*mut libc::posix_spawnattr_t, libc::c_int) -> libc::c_int;
    // SAFETY: libSystem is always loaded and the symbol has this C ABI.
    let symbol = unsafe {
        libc::dlsym(
            libc::RTLD_DEFAULT,
            c"responsibility_spawnattrs_setdisclaim".as_ptr(),
        )
    };
    if symbol.is_null() {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "libSystem lacks responsibility_spawnattrs_setdisclaim",
        ));
    }
    let function: Disclaim = unsafe { std::mem::transmute(symbol) };
    checked(unsafe { function(attr, 1) })
}

fn checked(rc: libc::c_int) -> io::Result<()> {
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(rc))
    }
}

/// Re-execute with the full argv (including argv[0]) unchanged, disclaiming
/// responsibility and retaining the PID, environment, descriptors and process group.
/// Call only before initialization or threads, while the environment is unchanged.
pub fn exec(program: &OsStr, args: impl Iterator<Item = OsString>) -> io::Error {
    struct Attr(libc::posix_spawnattr_t);
    impl Drop for Attr {
        fn drop(&mut self) {
            unsafe { libc::posix_spawnattr_destroy(&mut self.0) };
        }
    }
    // SAFETY: the attribute is exclusively owned until the process is replaced.
    unsafe {
        let mut raw = std::ptr::null_mut();
        if let Err(error) = checked(libc::posix_spawnattr_init(&mut raw)) {
            return error;
        }
        let mut attr = Attr(raw);
        if let Err(error) = disclaim(&mut attr.0).and_then(|()| {
            checked(libc::posix_spawnattr_setflags(
                &mut attr.0,
                libc::POSIX_SPAWN_SETEXEC as _,
            ))
        }) {
            return error;
        }
        let program = match CString::new(program.as_bytes()) {
            Ok(program) => program,
            Err(error) => return error.into(),
        };
        let args = match args
            .map(|s| CString::new(s.as_bytes()))
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(args) => args,
            Err(error) => return error.into(),
        };
        let pointers = |strings: &[CString]| {
            strings
                .iter()
                .map(|s| s.as_ptr().cast_mut())
                .chain(std::iter::once(std::ptr::null_mut()))
                .collect::<Vec<_>>()
        };
        let argv = pointers(&args);
        let rc = libc::posix_spawn(
            std::ptr::null_mut(),
            program.as_ptr(),
            std::ptr::null(),
            &attr.0,
            argv.as_ptr(),
            *libc::_NSGetEnviron(),
        );
        io::Error::from_raw_os_error(if rc == 0 { libc::EIO } else { rc })
    }
}

/// Spawn `program` with `argv` (argv[0] included) and the environment
/// `envp` ("NAME=value"), responsible for its own privacy requests, as
/// `std::process::Command` would spawn it otherwise: the descriptors not
/// closed on exec are inherited, `stdio` is dup'ed onto 0, 1 and 2 (None:
/// inherited), the signal mask is empty and SIGPIPE is default. A child
/// that disclaims needs no re-exec of its own to take responsibility.
pub fn spawn(
    program: &CStr,
    argv: &[CString],
    envp: &[CString],
    stdio: [Option<RawFd>; 3],
) -> io::Result<libc::pid_t> {
    struct Attr(libc::posix_spawnattr_t);
    impl Drop for Attr {
        fn drop(&mut self) {
            unsafe { libc::posix_spawnattr_destroy(&mut self.0) };
        }
    }
    struct Actions(libc::posix_spawn_file_actions_t);
    impl Drop for Actions {
        fn drop(&mut self) {
            unsafe { libc::posix_spawn_file_actions_destroy(&mut self.0) };
        }
    }
    let pointers = |strings: &[CString]| {
        strings
            .iter()
            .map(|s| s.as_ptr().cast_mut())
            .chain(std::iter::once(std::ptr::null_mut()))
            .collect::<Vec<_>>()
    };
    // SAFETY: the attribute and file actions are owned here and destroyed
    // after the spawn; the string arrays outlive it.
    unsafe {
        let mut raw = std::ptr::null_mut();
        checked(libc::posix_spawnattr_init(&mut raw))?;
        let mut attr = Attr(raw);
        let mut raw = std::ptr::null_mut();
        checked(libc::posix_spawn_file_actions_init(&mut raw))?;
        let mut actions = Actions(raw);
        disclaim(&mut attr.0)?;
        let mut mask: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut mask);
        checked(libc::posix_spawnattr_setsigmask(&mut attr.0, &mask))?;
        libc::sigaddset(&mut mask, libc::SIGPIPE);
        checked(libc::posix_spawnattr_setsigdefault(&mut attr.0, &mask))?;
        checked(libc::posix_spawnattr_setflags(
            &mut attr.0,
            (libc::POSIX_SPAWN_SETSIGMASK | libc::POSIX_SPAWN_SETSIGDEF) as _,
        ))?;
        for (target, fd) in stdio.iter().enumerate() {
            if let Some(fd) = fd {
                checked(libc::posix_spawn_file_actions_adddup2(
                    &mut actions.0,
                    *fd,
                    target as RawFd,
                ))?;
            }
        }
        let (argv, envp) = (pointers(argv), pointers(envp));
        let mut pid = 0;
        checked(libc::posix_spawn(
            &mut pid,
            program.as_ptr(),
            &actions.0,
            &attr.0,
            argv.as_ptr(),
            envp.as_ptr(),
        ))?;
        Ok(pid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    #[test]
    fn child_becomes_responsible_and_keeps_process_setup() {
        const CHILD: &str = "responsibility-child";
        let args: Vec<_> = std::env::args_os().collect();
        let program = std::env::current_exe().unwrap();
        if args.iter().any(|arg| arg == CHILD) {
            let pid = unsafe { libc::getpid() };
            if responsible_pid().unwrap() != pid {
                panic!("{}", exec(program.as_os_str(), args.into_iter()));
            }
            assert_eq!(responsible_pid().unwrap(), pid);
            assert_eq!(unsafe { libc::getpgrp() }, pid);
            assert_eq!(std::env::args_os().next().unwrap(), "custom-argv-zero");
            assert_eq!(
                std::env::vars_os().collect::<Vec<_>>(),
                [("AIM_TEST_DATA".into(), "preserved".into())]
            );
            println!("responsible={pid}");
            return;
        }
        let child = Command::new(&program)
            .arg0("custom-argv-zero")
            .args([
                "--exact",
                "tests::child_becomes_responsible_and_keeps_process_setup",
                "--nocapture",
                "--skip",
                CHILD,
            ])
            .env_clear()
            .env("AIM_TEST_DATA", "preserved")
            .process_group(0)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let pid = child.id();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains(&format!("responsible={pid}")));
    }

    #[test]
    fn a_spawned_child_is_responsible_with_its_stdio_and_environment() {
        const CHILD: &str = "spawned-responsible-child";
        let args: Vec<_> = std::env::args_os().collect();
        if args.iter().any(|arg| arg == CHILD) {
            let pid = unsafe { libc::getpid() };
            assert_eq!(responsible_pid().unwrap(), pid);
            assert_eq!(
                std::env::vars_os().collect::<Vec<_>>(),
                [("AIM_TEST_DATA".into(), "spawned".into())]
            );
            println!("responsible={pid}");
            return;
        }
        let program = std::env::current_exe().unwrap();
        let program = CString::new(program.as_os_str().as_bytes()).unwrap();
        let argv = [
            program.clone(),
            CString::new("--exact").unwrap(),
            CString::new("tests::a_spawned_child_is_responsible_with_its_stdio_and_environment")
                .unwrap(),
            CString::new("--nocapture").unwrap(),
            CString::new(CHILD).unwrap(),
        ];
        let envp = [CString::new("AIM_TEST_DATA=spawned").unwrap()];
        let mut fds = [0; 2];
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        let pid = spawn(&program, &argv, &envp, [None, Some(fds[1]), None]).unwrap();
        unsafe { libc::close(fds[1]) };
        let mut out = String::new();
        use std::io::Read;
        use std::os::fd::FromRawFd;
        unsafe { std::fs::File::from_raw_fd(fds[0]) }
            .read_to_string(&mut out)
            .unwrap();
        let mut status = 0;
        assert_eq!(unsafe { libc::waitpid(pid, &mut status, 0) }, pid);
        assert!(
            libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0,
            "{out}"
        );
        assert!(out.contains(&format!("responsible={pid}")), "{out}");
    }
}
