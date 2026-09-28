//! The module's side of the CoreAudio process ([`crate::io`]): started on
//! first use, asked with a timeout. A request that is not answered in time
//! gives the process up for good: its connection and lifeline close (so it
//! tears down whenever CoreAudio lets it) and the module's streams move to
//! the null sink.

use std::ffi::CString;
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::sync::Mutex;
use std::time::Duration;

use darwin_hostcall::audio::{FN_DEVICES, FN_OPEN};

use crate::io::{CONNECTION_FD, LIFELINE_FD};
use crate::wire::{self, Reply, Request};

pub enum Error {
    /// The CoreAudio process is not available (did not start, did not
    /// answer in time, or went away).
    Gone,
    /// It answered with this errno.
    Errno(i32),
}

struct Conn {
    sock: OwnedFd,
    _lifeline: OwnedFd,
    /// The CoreAudio process, a child of this one (0 in the tests).
    pid: libc::pid_t,
}

/// How the CoreAudio process ended, when it has.
fn exit_status(pid: libc::pid_t) -> String {
    let mut status = 0;
    // SAFETY: a non-blocking wait for our own child.
    match unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) } {
        p if p == pid && libc::WIFSIGNALED(status) => {
            format!("it was killed by signal {}", libc::WTERMSIG(status))
        }
        p if p == pid => format!("it exited with status {}", libc::WEXITSTATUS(status)),
        _ => "it closed the connection".into(),
    }
}

enum State {
    Unstarted,
    Up(Conn),
    Gone,
}

static STATE: Mutex<State> = Mutex::new(State::Unstarted);

/// How long a request may take. The first also starts the process, and
/// its device queries are the first contact with coreaudiod.
fn timeout(op: u32) -> Duration {
    match op {
        FN_DEVICES | FN_OPEN => Duration::from_secs(3),
        _ => Duration::from_secs(2),
    }
}

/// Send a request (with `fd` for an open) and wait for the reply.
pub fn request(req: &Request, fd: Option<i32>) -> Result<Reply, Error> {
    let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    if matches!(*state, State::Unstarted) {
        *state = match spawn() {
            Some(conn) => State::Up(conn),
            None => {
                crate::log(format_args!(
                    "cannot start the CoreAudio process; using the null sink"
                ));
                State::Gone
            }
        };
    }
    let State::Up(conn) = &*state else {
        return Err(Error::Gone);
    };
    let sock = conn.sock.as_fd();
    set_timeout(sock.as_raw_fd(), timeout(req.op));
    match wire::send(sock, req, fd).and_then(|()| wire::recv::<Reply>(sock)) {
        Ok(Some((reply, _))) if reply.status < 0 => Err(Error::Errno(-reply.status as i32)),
        Ok(Some((reply, _))) => Ok(reply),
        other => {
            let why = match other {
                Err(e) => e.to_string(),
                _ if conn.pid > 0 => {
                    std::thread::sleep(Duration::from_millis(50));
                    exit_status(conn.pid)
                }
                _ => "it closed the connection".into(),
            };
            crate::log(format_args!(
                "the CoreAudio process did not answer request {} ({why}); using the null sink",
                req.op
            ));
            *state = State::Gone;
            Err(Error::Gone)
        }
    }
}

fn set_timeout(fd: i32, t: Duration) {
    let tv = libc::timeval {
        tv_sec: t.as_secs() as _,
        tv_usec: t.subsec_micros() as _,
    };
    // SAFETY: setsockopt on our socket with a local timeval.
    unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_RCVTIMEO,
            (&tv as *const libc::timeval).cast(),
            size_of::<libc::timeval>() as libc::socklen_t,
        );
    }
}

/// Start `linux-run --audio-io` (this executable) with the connection's
/// other end and the lifeline's read end. It stays this process's child
/// (a guest never waits for it: the HAL does not fork) and outlives it
/// only for its teardown.
fn spawn() -> Option<Conn> {
    let exe = CString::new(std::env::current_exe().ok()?.as_os_str().as_bytes()).ok()?;
    let mut pair = [0; 2];
    let mut pipe = [0; 2];
    // SAFETY: plain libc calls on local arrays; every fd is owned below.
    unsafe {
        if libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, pair.as_mut_ptr()) != 0 {
            return None;
        }
        // The child's ends go high first, so that no dup2 below lands on a
        // descriptor another one still needs.
        let high = |fd: i32| match libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 10) {
            -1 => OwnedFd::from_raw_fd(fd),
            h => {
                libc::close(fd);
                OwnedFd::from_raw_fd(h)
            }
        };
        let (mine, theirs) = (OwnedFd::from_raw_fd(pair[0]), high(pair[1]));
        if libc::pipe(pipe.as_mut_ptr()) != 0 {
            return None;
        }
        let (read_end, write_end) = (high(pipe[0]), OwnedFd::from_raw_fd(pipe[1]));
        for fd in [&mine, &write_end] {
            libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC);
        }
        let on: libc::c_int = 1;
        libc::setsockopt(
            mine.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_NOSIGPIPE,
            (&on as *const libc::c_int).cast(),
            size_of::<libc::c_int>() as libc::socklen_t,
        );

        let mut actions: libc::posix_spawn_file_actions_t = std::ptr::null_mut();
        let mut attr: libc::posix_spawnattr_t = std::ptr::null_mut();
        libc::posix_spawn_file_actions_init(&mut actions);
        libc::posix_spawnattr_init(&mut attr);
        let null = c"/dev/null";
        libc::posix_spawn_file_actions_addopen(&mut actions, 0, null.as_ptr(), libc::O_RDONLY, 0);
        libc::posix_spawn_file_actions_addopen(&mut actions, 1, null.as_ptr(), libc::O_WRONLY, 0);
        let err = libc::fcntl(crate::log_fd(), libc::F_DUPFD_CLOEXEC, 10);
        if err >= 0 {
            libc::posix_spawn_file_actions_adddup2(&mut actions, err, 2);
        } else {
            libc::posix_spawn_file_actions_addopen(
                &mut actions,
                2,
                null.as_ptr(),
                libc::O_WRONLY,
                0,
            );
        }
        libc::posix_spawn_file_actions_adddup2(&mut actions, theirs.as_raw_fd(), CONNECTION_FD);
        libc::posix_spawn_file_actions_adddup2(&mut actions, read_end.as_raw_fd(), LIFELINE_FD);
        let mut all: libc::sigset_t = 0;
        let mut none: libc::sigset_t = 0;
        libc::sigfillset(&mut all);
        libc::sigemptyset(&mut none);
        libc::posix_spawnattr_setsigdefault(&mut attr, &all);
        libc::posix_spawnattr_setsigmask(&mut attr, &none);
        // A process group of its own: the service's group may be killed.
        libc::posix_spawnattr_setpgroup(&mut attr, 0);
        libc::posix_spawnattr_setflags(
            &mut attr,
            (libc::POSIX_SPAWN_CLOEXEC_DEFAULT
                | libc::POSIX_SPAWN_SETPGROUP
                | libc::POSIX_SPAWN_SETSIGDEF
                | libc::POSIX_SPAWN_SETSIGMASK) as _,
        );
        let argv = [
            c"linux-run".as_ptr(),
            c"--audio-io".as_ptr(),
            std::ptr::null(),
        ];
        let envp: [*const libc::c_char; 1] = [std::ptr::null()];
        let mut pid = 0;
        let rc = libc::posix_spawn(
            &mut pid,
            exe.as_ptr(),
            &actions,
            &attr,
            argv.as_ptr().cast(),
            envp.as_ptr().cast(),
        );
        libc::posix_spawn_file_actions_destroy(&mut actions);
        libc::posix_spawnattr_destroy(&mut attr);
        if err >= 0 {
            libc::close(err);
        }
        if rc != 0 {
            return None;
        }
        Some(Conn {
            sock: mine,
            _lifeline: write_end,
            pid,
        })
    }
}

/// The module's tests serve requests from a thread of their own process.
#[cfg(test)]
pub fn serve_in_thread() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(serve_in_thread_once);
}

#[cfg(test)]
fn serve_in_thread_once() {
    let mut pair = [0; 2];
    // SAFETY: a socketpair into a local array; both ends are owned below.
    let (mine, theirs) = unsafe {
        assert_eq!(
            libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, pair.as_mut_ptr()),
            0
        );
        (OwnedFd::from_raw_fd(pair[0]), OwnedFd::from_raw_fd(pair[1]))
    };
    std::thread::spawn(move || crate::io::serve(theirs.as_fd()));
    let lifeline = mine.try_clone().unwrap();
    *STATE.lock().unwrap() = State::Up(Conn {
        sock: mine,
        _lifeline: lifeline,
        pid: 0,
    });
}
