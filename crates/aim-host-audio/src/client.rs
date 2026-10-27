//! The module's side of the CoreAudio process ([`crate::io`]): started on
//! first use, asked with a timeout. A request that is not answered in time
//! gives the process up: its connection and lifeline close (so it tears
//! down whenever CoreAudio lets it) and the module's streams move to the
//! null sink. A device query or an open after a back-off starts a new
//! process; streams opened on it play on it, and a stream of a process
//! given up stays on the null sink.

use std::ffi::CString;
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use aim_hostcall::audio::{FN_DEVICES, FN_OPEN};

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
    /// Which process this is, counting from 1: a stream's requests go to
    /// the process it was opened on.
    generation: u64,
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
    /// Given up; a query or an open from `retry_at` starts a new process.
    Gone {
        retry_at: Instant,
    },
}

/// The first wait before a process given up is started again, and the
/// longest: each give-up doubles it, so a coreaudiod that keeps hanging
/// costs an open's timeout at most every few minutes.
const FIRST_RETRY: Duration = Duration::from_secs(5);
const LONGEST_RETRY: Duration = Duration::from_secs(300);

struct Client {
    state: State,
    /// The last process's generation.
    generation: u64,
    /// The wait after the next give-up.
    retry: Duration,
    /// Processes given up and not yet reaped.
    exited: Vec<libc::pid_t>,
}

impl Client {
    const fn new() -> Self {
        Client {
            state: State::Unstarted,
            generation: 0,
            retry: FIRST_RETRY,
            exited: Vec::new(),
        }
    }

    /// The connection a request goes to: the process `generation` for a
    /// stream's, else the running one, started if there is none and it is
    /// time to.
    fn conn(
        &mut self,
        generation: Option<u64>,
        now: Instant,
        start: impl FnOnce() -> Option<Conn>,
    ) -> Option<&Conn> {
        let start_now = match self.state {
            State::Unstarted => true,
            State::Gone { retry_at } => generation.is_none() && now >= retry_at,
            State::Up(_) => false,
        };
        if start_now {
            if self.generation > 0 {
                crate::log(format_args!("starting the CoreAudio process again"));
            }
            self.reap();
            match start() {
                Some(mut conn) => {
                    self.generation += 1;
                    conn.generation = self.generation;
                    self.state = State::Up(conn);
                }
                None => {
                    crate::log(format_args!(
                        "cannot start the CoreAudio process; using the null sink"
                    ));
                    self.give_up(now);
                }
            }
        }
        match &self.state {
            State::Up(conn) if generation.is_none_or(|g| g == conn.generation) => Some(conn),
            _ => None,
        }
    }

    /// Close the running process, if any, and wait before starting another.
    fn give_up(&mut self, now: Instant) {
        if let State::Up(conn) = &self.state
            && conn.pid > 0
        {
            self.exited.push(conn.pid);
        }
        self.state = State::Gone {
            retry_at: now + self.retry,
        };
        self.retry = (self.retry * 2).min(LONGEST_RETRY);
    }

    /// Reap the processes given up that have exited.
    fn reap(&mut self) {
        self.exited.retain(|&pid| {
            let mut status = 0;
            // SAFETY: a non-blocking wait for our own child.
            unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) == 0 }
        });
    }

    /// The generation of the running process.
    fn running(&self) -> Option<u64> {
        match &self.state {
            State::Up(conn) => Some(conn.generation),
            _ => None,
        }
    }
}

static STATE: Mutex<Client> = Mutex::new(Client::new());

/// The generation of the running CoreAudio process, if one runs.
pub fn running() -> Option<u64> {
    STATE.lock().unwrap_or_else(|e| e.into_inner()).running()
}

/// How long a request may take. The first also starts the process, and
/// its device queries are the first contact with coreaudiod.
fn timeout(op: u32) -> Duration {
    match op {
        FN_DEVICES | FN_OPEN => Duration::from_secs(3),
        _ => Duration::from_secs(2),
    }
}

/// Send a request (with `fd` for an open) and wait for the reply: for a
/// stream's request, to the process `generation` it was opened on; else to
/// the running process. The reply, and the generation that answered.
pub fn request(
    req: &Request,
    fd: Option<i32>,
    generation: Option<u64>,
) -> Result<(Reply, u64), Error> {
    let mut client = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    let Some(conn) = client.conn(generation, now, spawn) else {
        return Err(Error::Gone);
    };
    let sock = conn.sock.as_fd();
    set_timeout(sock.as_raw_fd(), timeout(req.op));
    let why = match wire::send(sock, req, fd).and_then(|()| wire::recv::<Reply>(sock)) {
        Ok(Some((reply, _))) if reply.status < 0 => {
            return Err(Error::Errno(-reply.status as i32));
        }
        Ok(Some((reply, _))) => return Ok((reply, conn.generation)),
        Err(e) => e.to_string(),
        Ok(None) if conn.pid > 0 => {
            std::thread::sleep(Duration::from_millis(50));
            exit_status(conn.pid)
        }
        Ok(None) => "it closed the connection".into(),
    };
    crate::log(format_args!(
        "the CoreAudio process did not answer request {} ({why}); using the null sink for {} s",
        req.op,
        client.retry.as_secs()
    ));
    client.give_up(now);
    Err(Error::Gone)
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
        if let Err(error) = aim_process::disclaim(&mut attr) {
            libc::posix_spawn_file_actions_destroy(&mut actions);
            libc::posix_spawnattr_destroy(&mut attr);
            crate::log(format_args!("CoreAudio process responsibility: {error}"));
            return None;
        }
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
            generation: 0,
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
    let mut client = STATE.lock().unwrap();
    client.generation = 1;
    client.state = State::Up(Conn {
        sock: mine,
        _lifeline: lifeline,
        pid: 0,
        generation: 1,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A connection to nothing, standing for a started process's.
    fn conn() -> Option<Conn> {
        let mut pair = [0; 2];
        // SAFETY: a socketpair into a local array; both ends are owned.
        unsafe {
            assert_eq!(
                libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, pair.as_mut_ptr()),
                0
            );
            Some(Conn {
                sock: OwnedFd::from_raw_fd(pair[0]),
                _lifeline: OwnedFd::from_raw_fd(pair[1]),
                pid: 0,
                generation: 0,
            })
        }
    }

    fn generation(c: Option<&Conn>) -> Option<u64> {
        c.map(|c| c.generation)
    }

    #[test]
    fn a_process_given_up_is_started_again_after_a_back_off() {
        let mut c = Client::new();
        let t0 = Instant::now();
        assert_eq!(generation(c.conn(None, t0, conn)), Some(1));
        c.give_up(t0);
        // Neither a stream of the first process nor an open before the
        // back-off starts one.
        assert!(
            c.conn(Some(1), t0 + FIRST_RETRY, || unreachable!())
                .is_none()
        );
        assert!(
            c.conn(None, t0 + FIRST_RETRY / 2, || unreachable!())
                .is_none()
        );
        let t1 = t0 + FIRST_RETRY;
        assert_eq!(generation(c.conn(None, t1, conn)), Some(2));
        assert_eq!(c.running(), Some(2));
        // The first process's streams stay on the null sink.
        assert!(c.conn(Some(1), t1, || unreachable!()).is_none());
        assert_eq!(generation(c.conn(Some(2), t1, || unreachable!())), Some(2));
        // Each give-up doubles the wait.
        c.give_up(t1);
        assert!(c.conn(None, t1 + FIRST_RETRY, || unreachable!()).is_none());
        assert_eq!(
            generation(c.conn(None, t1 + FIRST_RETRY * 2, conn)),
            Some(3)
        );
    }

    #[test]
    fn a_failed_start_waits_too_and_the_wait_is_bounded() {
        let mut c = Client::new();
        let mut now = Instant::now();
        assert!(c.conn(None, now, || None).is_none());
        let mut waits = Vec::new();
        for _ in 0..10 {
            let State::Gone { retry_at } = c.state else {
                panic!("not given up")
            };
            waits.push(retry_at - now);
            now = retry_at;
            assert!(c.conn(None, now, || None).is_none());
        }
        assert_eq!(waits[..2], [FIRST_RETRY, FIRST_RETRY * 2]);
        assert_eq!(waits.last(), Some(&LONGEST_RETRY));
        assert_eq!(c.running(), None);
    }
}
