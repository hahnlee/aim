use super::*;
use std::{
    fs, io,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::fs::DirBuilderExt,
    },
    time::Duration,
};
/// How long an actor may take to answer; it never waits for a signal.
const ANSWER: Duration = Duration::from_secs(10);
const PENDING: u8 = b'p';
const EXIT: u8 = b'x';
fn hangup_set() -> libc::sigset_t {
    let mut set = 0;
    unsafe {
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGHUP);
        libc::sigaddset(&mut set, libc::SIGCONT);
    }
    set
}
fn pipe() -> [OwnedFd; 2] {
    let mut fds = [0; 2];
    assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
    fds.map(|fd| {
        assert_eq!(
            unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) },
            0
        );
        unsafe { OwnedFd::from_raw_fd(fd) }
    })
}
/// Close every descriptor of this forked child but `keep`, as closefrom
/// would: a fork without exec keeps even close-on-exec ones, and a
/// concurrent test waiting for a last close must not wait for an actor.
unsafe fn close_others(keep: [i32; 2]) {
    let mut fds = [libc::proc_fdinfo {
        proc_fd: 0,
        proc_fdtype: 0,
    }; 256];
    loop {
        let size = unsafe {
            libc::proc_pidinfo(
                libc::getpid(),
                libc::PROC_PIDLISTFDS,
                0,
                fds.as_mut_ptr().cast(),
                std::mem::size_of_val(&fds) as i32,
            )
        };
        if size <= 0 {
            unsafe { libc::_exit(5) };
        }
        let listed = &fds[..size as usize / std::mem::size_of::<libc::proc_fdinfo>()];
        let mut closed = false;
        for fd in listed
            .iter()
            .map(|f| f.proc_fd)
            .filter(|fd| !keep.contains(fd))
        {
            unsafe { libc::close(fd) };
            closed = true;
        }
        if !closed {
            return;
        }
    }
}
/// The actor's side of the fork: async-signal-safe calls only, since the
/// test process is multi-threaded. It blocks SIGHUP and SIGCONT with their
/// default actions, because Darwin discards a blocked signal whose action
/// is SIG_IGN (as `nohup` leaves SIGHUP), and answers each command with its
/// pending set, so no read waits for a signal to arrive.
unsafe fn serve(leader: bool, parent: libc::pid_t, commands: i32, answers: i32) -> ! {
    unsafe {
        close_others([commands, answers]);
        let set = hangup_set();
        if leader && libc::setsid() < 0
            || libc::sigprocmask(libc::SIG_BLOCK, &set, std::ptr::null_mut()) != 0
            || libc::signal(libc::SIGHUP, libc::SIG_DFL) == libc::SIG_ERR
            || libc::signal(libc::SIGCONT, libc::SIG_DFL) == libc::SIG_ERR
        {
            libc::_exit(2);
        }
        loop {
            let mut pending = 0;
            libc::sigpending(&mut pending);
            pending &= set;
            let size = std::mem::size_of_val(&pending);
            if libc::write(answers, (&raw const pending).cast(), size) != size as isize {
                libc::_exit(3);
            }
            // A session leader escapes the process-group kill of a timed-out
            // test binary: it leaves with its parent instead.
            let mut command = libc::pollfd {
                fd: commands,
                events: libc::POLLIN,
                revents: 0,
            };
            loop {
                let polled = libc::poll(&mut command, 1, 100);
                if polled > 0 {
                    break;
                }
                if polled < 0 && *libc::__error() != libc::EINTR || libc::getppid() != parent {
                    libc::_exit(4);
                }
            }
            let mut command = 0u8;
            if libc::read(commands, (&raw mut command).cast(), 1) != 1 || command != PENDING {
                libc::_exit(0);
            }
        }
    }
}
/// A real process that the owner signals.
struct Actor {
    commands: OwnedFd,
    answers: OwnedFd,
    identity: ProcessIdentity,
    reaped: bool,
}
impl Actor {
    fn new(leader: bool) -> Self {
        let [command_reader, commands] = pipe();
        let [answers, answer_writer] = pipe();
        let parent = unsafe { libc::getpid() };
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0, "fork: {}", io::Error::last_os_error());
        if pid == 0 {
            unsafe {
                serve(
                    leader,
                    parent,
                    command_reader.as_raw_fd(),
                    answer_writer.as_raw_fd(),
                )
            }
        }
        let mut actor = Self {
            commands,
            answers,
            identity: ProcessIdentity {
                host_pid: pid,
                start_seconds: 0,
                start_microseconds: 0,
            },
            reaped: false,
        };
        drop((command_reader, answer_writer));
        assert_eq!(actor.answer(), 0, "SIGHUP or SIGCONT pending at start");
        actor.identity = ProcessIdentity::running(pid).unwrap();
        println!(
            "ACTUAL_ACTOR {:?} sid={} pgid={}",
            actor.identity,
            unsafe { libc::getsid(pid) },
            unsafe { libc::getpgid(pid) },
        );
        actor
    }
    fn command(&self, command: u8) {
        assert_eq!(
            unsafe { libc::write(self.commands.as_raw_fd(), (&raw const command).cast(), 1) },
            1,
            "command to actor {:?}: {}",
            self.identity,
            io::Error::last_os_error()
        );
    }
    /// The actor's pending SIGHUP and SIGCONT.
    fn pending(&self) -> libc::sigset_t {
        self.command(PENDING);
        self.answer()
    }
    fn answer(&self) -> libc::sigset_t {
        let mut ready = libc::pollfd {
            fd: self.answers.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let polled = loop {
            let polled = unsafe { libc::poll(&mut ready, 1, ANSWER.as_millis() as i32) };
            if polled >= 0 || io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
                break polled;
            }
        };
        assert!(
            polled > 0,
            "actor {:?} did not answer within {ANSWER:?}: poll {polled}, {}",
            self.identity,
            io::Error::last_os_error()
        );
        let mut pending: libc::sigset_t = 0;
        let size = std::mem::size_of_val(&pending);
        let read = unsafe { libc::read(self.answers.as_raw_fd(), (&raw mut pending).cast(), size) };
        assert_eq!(
            read, size as isize,
            "actor {:?} ended without answering",
            self.identity
        );
        pending
    }
    fn exit(&mut self) {
        self.command(EXIT);
        let mut status = 0;
        assert_eq!(
            unsafe { libc::waitpid(self.identity.host_pid, &mut status, 0) },
            self.identity.host_pid
        );
        self.reaped = true;
        assert!(
            libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0,
            "actor {:?} status {status:#x}",
            self.identity
        );
        assert!(!self.identity.is_live());
        println!("REAPED {:?}", self.identity);
    }
}
impl Drop for Actor {
    fn drop(&mut self) {
        if !self.reaped {
            unsafe {
                libc::kill(self.identity.host_pid, libc::SIGKILL);
                libc::waitpid(self.identity.host_pid, std::ptr::null_mut(), 0);
            }
        }
    }
}
struct Cleanup(PathBuf);
impl Drop for Cleanup {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
struct Fixture {
    table: PathBuf,
    pair: Arc<Pair>,
    sessions: Sessions,
    _cleanup: Cleanup,
}
impl Fixture {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "aim-pty-session-independent-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let table = root.join("by-pid");
        fs::create_dir(&table).unwrap();
        let pair = Arc::new(Pair::allocate(&root).unwrap());
        let mut sessions = Sessions::new();
        sessions.set_table(table.clone());
        Self {
            table,
            pair,
            sessions,
            _cleanup: Cleanup(root),
        }
    }
    fn register(&self, actor: &Actor, cap: u64) {
        crate::process_namespace::register_mount_namespace(
            &self.table,
            actor.identity,
            "fixture-actual-child",
        )
        .unwrap();
        fs::write(
            self.table.join(actor.identity.host_pid.to_string()),
            format!(
                "uid\t{}\neuid\t{}\ncap_effective\t0x{cap:x}\n",
                unsafe { libc::getuid() },
                unsafe { libc::geteuid() }
            ),
        )
        .unwrap();
        let observed = read_credentials(&self.table, actor.identity).unwrap();
        assert_eq!(observed.uid, unsafe { libc::getuid() });
        assert_eq!(observed.effective_uid, unsafe { libc::geteuid() });
        assert_eq!(observed.effective_capabilities, cap);
    }
}
#[test]
fn forced_steal_requires_registered_cap_and_foreign_session_is_rejected() {
    let mut f = Fixture::new();
    let mut original = Actor::new(true);
    let mut thief = Actor::new(true);
    let mut nonleader = Actor::new(false);
    f.register(&original, 0);
    f.register(&thief, 0);
    f.register(&nonleader, 1 << 21);
    assert_eq!(
        unsafe { libc::getsid(original.identity.host_pid) },
        original.identity.host_pid
    );
    assert!(
        f.sessions
            .claim(&f.pair, original.identity, false, false, true)
            .unwrap()
    );
    assert_eq!(
        f.sessions
            .claim(&f.pair, thief.identity, true, false, true)
            .unwrap_err()
            .raw_os_error(),
        Some(libc::EPERM)
    );
    assert_eq!(
        f.sessions
            .state(&f.pair, original.identity, false)
            .unwrap()
            .0,
        original.identity.host_pid
    );
    assert_eq!(
        f.sessions
            .state(&f.pair, thief.identity, false)
            .unwrap_err()
            .raw_os_error(),
        Some(libc::ENOTTY)
    );
    assert_eq!(
        f.sessions
            .claim(&f.pair, nonleader.identity, true, false, true)
            .unwrap_err()
            .raw_os_error(),
        Some(libc::EPERM)
    );
    f.register(&thief, 1 << 21);
    assert_eq!(
        f.sessions
            .claim(&f.pair, thief.identity, false, false, true)
            .unwrap_err()
            .raw_os_error(),
        Some(libc::EPERM)
    );
    assert!(
        f.sessions
            .claim(&f.pair, thief.identity, true, false, true)
            .unwrap()
    );
    assert_eq!(
        f.sessions
            .state(&f.pair, original.identity, false)
            .unwrap_err()
            .raw_os_error(),
        Some(libc::ENOTTY)
    );
    assert_eq!(
        f.sessions.state(&f.pair, thief.identity, false).unwrap().0,
        thief.identity.host_pid
    );
    original.exit();
    nonleader.exit();
    thief.exit();
    f.sessions.leader_exit(thief.identity.host_pid).unwrap();
}
#[test]
fn leader_exit_releases_binding_and_live_leader_is_not_retired() {
    let mut f = Fixture::new();
    let mut original = Actor::new(true);
    let mut successor = Actor::new(true);
    f.register(&original, 0);
    f.register(&successor, 0);
    assert!(
        f.sessions
            .claim(&f.pair, original.identity, false, false, true)
            .unwrap()
    );
    f.sessions.leader_exit(original.identity.host_pid).unwrap();
    assert_eq!(
        f.sessions
            .state(&f.pair, original.identity, false)
            .unwrap()
            .0,
        original.identity.host_pid
    );
    original.exit();
    f.sessions.leader_exit(original.identity.host_pid).unwrap();
    assert_eq!(
        f.sessions
            .state(&f.pair, successor.identity, true)
            .unwrap_err()
            .raw_os_error(),
        Some(libc::ENOTTY)
    );
    assert!(
        f.sessions
            .claim(&f.pair, successor.identity, false, false, true)
            .unwrap()
    );
    successor.exit();
    f.sessions.leader_exit(successor.identity.host_pid).unwrap();
}
#[test]
fn actual_owned_leader_detach_delivers_native_hup_and_cont() {
    let mut f = Fixture::new();
    let mut leader = Actor::new(true);
    f.register(&leader, 0);
    assert!(
        f.sessions
            .claim(&f.pair, leader.identity, false, false, true)
            .unwrap()
    );
    f.sessions.detach(&f.pair, leader.identity).unwrap();
    assert_eq!(leader.pending(), hangup_set());
    assert_eq!(
        f.sessions
            .state(&f.pair, leader.identity, false)
            .unwrap_err()
            .raw_os_error(),
        Some(libc::ENOTTY)
    );
    leader.exit();
}
#[test]
fn retained_binding_signals_after_credential_directory_teardown_without_adopting_foreign_actor() {
    let mut f = Fixture::new();
    let mut leader = Actor::new(true);
    let mut foreign = Actor::new(true);
    f.register(&leader, 0);
    f.register(&foreign, 0);
    assert!(
        f.sessions
            .claim(&f.pair, leader.identity, false, false, true)
            .unwrap()
    );
    f.sessions.bindings[0].members.push(foreign.identity);
    let mut stale = leader.identity;
    stale.start_microseconds += 1;
    f.sessions.bindings[0].members.push(stale);
    fs::remove_dir_all(&f.table).unwrap();
    assert_eq!(
        f.sessions
            .claim(&f.pair, foreign.identity, true, false, true)
            .unwrap_err()
            .raw_os_error(),
        Some(libc::ENOENT)
    );
    f.sessions.master_closed(&f.pair).unwrap();
    assert_eq!(leader.pending(), hangup_set());
    assert_eq!(foreign.pending(), 0);
    assert!(f.sessions.bindings.is_empty());
    leader.exit();
    foreign.exit();
}
#[test]
fn actor_keeps_no_descriptor_of_the_test_process() {
    let [_reader, _writer] = pipe();
    let mut actor = Actor::new(false);
    let mut fds = [libc::proc_fdinfo {
        proc_fd: 0,
        proc_fdtype: 0,
    }; 16];
    let size = unsafe {
        libc::proc_pidinfo(
            actor.identity.host_pid,
            libc::PROC_PIDLISTFDS,
            0,
            fds.as_mut_ptr().cast(),
            std::mem::size_of_val(&fds) as i32,
        )
    };
    // Its command and answer pipes only.
    assert_eq!(
        size as usize / std::mem::size_of::<libc::proc_fdinfo>(),
        2,
        "{:?}",
        io::Error::last_os_error()
    );
    actor.exit();
}
