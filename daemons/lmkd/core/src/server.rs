// SPDX-License-Identifier: Apache-2.0
// The command handling is ported from AOSP lmkd (platform/system/memory/lmkd
// at android-16.0.0_r1: lmkd.cpp, ctrl_command_handler, apply_proc_prio,
// kill_one_process), Copyright (C) 2013 The Android Open Source Project,
// Apache-2.0.

//! The control socket and the kill loop, one thread polling everything as
//! the original's main loop does: the listening socket, up to three client
//! connections and the pressure source.
//!
//! While the host is under pressure lmkd re-reads it every `interval` and
//! kills at most one process each time, so that the kill's effect shows in
//! the host's level before the next one.

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::time::{Duration, Instant};

use crate::policy::{self, Level, Memory, Procs, Target};
use crate::proto::*;

/// Where lmkd learns about memory pressure.
pub trait Pressure {
    /// A descriptor that becomes readable when the level may have changed.
    fn fd(&self) -> RawFd;
    /// Drain [`Pressure::fd`] and return the memory now.
    fn read(&mut self) -> Memory;
}

/// `MAX_DATA_CONN`.
const MAX_CONNECTIONS: usize = 3;

struct Connection {
    fd: OwnedFd,
    /// Bits `1 << LMK_ASYNC_EVENT_*` the client subscribed to.
    events: u32,
}

pub struct Server<P> {
    listener: OwnedFd,
    connections: Vec<Connection>,
    pressure: P,
    procs: Procs,
    targets: Vec<Target>,
    boot_completed: bool,
    interval: Duration,
    level: Level,
    last_check: Option<Instant>,
}

impl<P: Pressure> Server<P> {
    /// Serve `listener`, a listening `SOCK_SEQPACKET` socket.
    pub fn new(listener: OwnedFd, pressure: P, interval: Duration) -> Self {
        Server {
            listener,
            connections: Vec::new(),
            pressure,
            procs: Procs::default(),
            targets: Vec::new(),
            boot_completed: false,
            interval,
            level: Level::Normal,
            last_check: None,
        }
    }

    /// Serve `fd`, a connected message socket, as if it had been accepted.
    pub fn adopt(&mut self, fd: OwnedFd) {
        self.connections.push(Connection { fd, events: 0 });
    }

    /// Serve until polling fails.
    pub fn run(mut self) -> io::Error {
        self.check();
        loop {
            let mut fds: Vec<libc::pollfd> = [self.listener.as_raw_fd(), self.pressure.fd()]
                .into_iter()
                .chain(self.connections.iter().map(|c| c.fd.as_raw_fd()))
                .map(|fd| libc::pollfd {
                    fd,
                    events: libc::POLLIN,
                    revents: 0,
                })
                .collect();
            let timeout = match (self.level, self.last_check) {
                (Level::Normal, _) | (_, None) => -1,
                (_, Some(t)) => self.interval.saturating_sub(t.elapsed()).as_millis() as i32,
            };
            // SAFETY: poll on our own array.
            let n = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, timeout) };
            if n < 0 {
                let e = io::Error::last_os_error();
                if e.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return e;
            }
            let ready = |i: usize| fds[i].revents != 0;
            // Clients first, so a registration made just before the host's
            // level rose is known to the kill.
            let mut closed = Vec::new();
            for i in 0..self.connections.len() {
                if ready(2 + i) && !self.serve(i) {
                    closed.push(i);
                }
            }
            for i in closed.into_iter().rev() {
                self.connections.remove(i);
            }
            if ready(0) {
                self.accept();
            }
            let due = self.level != Level::Normal
                && self.last_check.is_none_or(|t| t.elapsed() >= self.interval);
            if ready(1) || due {
                self.check();
            }
        }
    }

    fn accept(&mut self) {
        if self.connections.len() == MAX_CONNECTIONS {
            // As the original: drop them all rather than let idle clients
            // lock ActivityManager out; it reconnects at once.
            log::warn!("too many lmkd connections; dropping them");
            self.connections.clear();
        }
        // SAFETY: accept on our listening socket.
        let fd = unsafe {
            libc::accept(
                self.listener.as_raw_fd(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if fd < 0 {
            log::error!(
                "lmkd control socket accept failed: {}",
                io::Error::last_os_error()
            );
            return;
        }
        log::info!("lmkd data connection established");
        self.connections.push(Connection {
            // SAFETY: a fresh connection, owned from here on.
            fd: unsafe { OwnedFd::from_raw_fd(fd) },
            events: 0,
        });
    }

    /// Handle one packet of connection `i`; false when it has closed.
    fn serve(&mut self, i: usize) -> bool {
        let mut buf = [0u8; CTRL_PACKET_MAX_SIZE];
        let fd = self.connections[i].fd.as_raw_fd();
        // SAFETY: a receive into our buffer.
        let n = unsafe { libc::recv(fd, buf.as_mut_ptr().cast(), buf.len(), 0) };
        if n < 0 {
            return io::Error::last_os_error().kind() == io::ErrorKind::Interrupted;
        }
        if n == 0 {
            log::info!("lmkd data connection dropped");
            return false;
        }
        if (n as usize) < 4 {
            log::error!("wrong control socket read length len={n}");
            return true;
        }
        let words = decode(&buf[..n as usize]);
        if let Some(reply) = self.command(i, &words) {
            send(fd, &encode(&reply));
        }
        true
    }

    /// Run a command from connection `i`; returns its reply, if it has one.
    fn command(&mut self, i: usize, words: &[i32]) -> Option<Vec<i32>> {
        let (cmd, args) = (words[0], &words[1..]);
        let wrong = || {
            log::error!(
                "wrong control socket read length cmd={cmd} len={}",
                words.len() * 4
            );
            None
        };
        match cmd {
            LMK_TARGET => {
                if args.len() % 2 != 0 || args.is_empty() || args.len() / 2 > MAX_TARGETS {
                    return wrong();
                }
                self.targets = args
                    .chunks_exact(2)
                    .map(|t| Target {
                        minfree: t[0],
                        adj: t[1],
                    })
                    .collect();
                let levels: Vec<String> = self
                    .targets
                    .iter()
                    .map(|t| format!("{}:{}", t.minfree, t.adj))
                    .collect();
                log::info!("minfree levels {}", levels.join(","));
                None
            }
            LMK_PROCPRIO => {
                if !(3..=4).contains(&args.len()) {
                    return wrong();
                }
                // The process type is optional; an app when missing.
                self.procprio(args[0], args[1] as u32, args[2], *args.get(3).unwrap_or(&0));
                None
            }
            LMK_PROCS_PRIO => {
                if args.is_empty() || args.len() % PROCPRIO_FIELDS != 0 {
                    log::error!("LMK_PROCS_PRIO received invalid packet format");
                    return None;
                }
                for p in args.chunks_exact(PROCPRIO_FIELDS) {
                    self.procprio(p[0], p[1] as u32, p[2], p[3]);
                }
                None
            }
            LMK_PROCREMOVE => {
                if args.len() != 1 {
                    return wrong();
                }
                self.procs.remove(args[0]);
                None
            }
            LMK_PROCPURGE => {
                if !args.is_empty() {
                    return wrong();
                }
                self.procs.purge();
                None
            }
            LMK_GETKILLCNT => {
                if args.len() != 2 {
                    return wrong();
                }
                Some(vec![
                    LMK_GETKILLCNT,
                    self.procs.kill_count(args[0], args[1]) as i32,
                ])
            }
            LMK_SUBSCRIBE => {
                if args.len() != 1 || !(0..32).contains(&args[0]) {
                    return wrong();
                }
                self.connections[i].events |= 1 << args[0];
                None
            }
            LMK_UPDATE_PROPS => {
                if !args.is_empty() {
                    return wrong();
                }
                Some(vec![LMK_UPDATE_PROPS, 0])
            }
            LMK_START_MONITORING => None,
            LMK_BOOT_COMPLETED => {
                if !args.is_empty() {
                    return wrong();
                }
                // 1: already handled.
                let r = self.boot_completed as i32;
                self.boot_completed = true;
                Some(vec![LMK_BOOT_COMPLETED, r])
            }
            _ => {
                log::error!("received unknown command code {cmd}");
                None
            }
        }
    }

    fn procprio(&mut self, pid: i32, uid: u32, adj: i32, ptype: i32) {
        if !(policy::OOM_SCORE_ADJ_MIN..=policy::OOM_SCORE_ADJ_MAX).contains(&adj) {
            log::error!("invalid PROCPRIO oomadj argument {adj}");
            return;
        }
        if !(PROC_TYPE_APP..=PROC_TYPE_SERVICE).contains(&ptype) {
            log::error!("invalid PROCPRIO process type argument {ptype}");
            return;
        }
        if let Some(s) = status(pid)
            && s.tgid != pid
        {
            log::error!(
                "attempt to register a task that is not a thread group leader (tid {pid}, tgid {})",
                s.tgid
            );
            return;
        }
        self.procs.set(pid, uid, adj);
    }

    /// Read the host's memory and kill once if it calls for it.
    fn check(&mut self) {
        let mem = self.pressure.read();
        if mem.level != self.level {
            log::info!("memory pressure {:?} -> {:?}", self.level, mem.level);
            self.level = mem.level;
        }
        self.last_check = Some(Instant::now());
        if let Some(min_adj) = policy::min_score_adj(&mem, &self.targets) {
            self.kill(min_adj, &mem);
        }
    }

    /// Kill the first victim at or above `min_adj` that can be killed.
    fn kill(&mut self, min_adj: i32, mem: &Memory) {
        while let Some(v) = self
            .procs
            .victim(min_adj, |pid| status(pid).map(|s| s.rss_kb))
        {
            self.procs.remove(v.pid);
            let st = status(v.pid);
            if let Some(s) = &st
                && s.tgid != v.pid
            {
                log::error!(
                    "possible pid reuse detected (pid {}, tgid {})",
                    v.pid,
                    s.tgid
                );
                continue;
            }
            // SAFETY: plain kill.
            if unsafe { libc::kill(v.pid, libc::SIGKILL) } != 0 {
                log::error!("kill({}): {}", v.pid, io::Error::last_os_error());
                continue;
            }
            self.procs.killed(v.adj);
            let rss_kb = st.map_or(0, |s| s.rss_kb);
            let name = task_name(v.pid);
            log::info!(
                "Kill '{}' ({}), uid {}, oom_score_adj {} to free {rss_kb}kB rss; reason: {:?} memory pressure",
                name.as_deref().unwrap_or(""),
                v.pid,
                v.uid,
                v.adj,
                mem.level,
            );
            let kill = prockill(v.pid, v.uid, rss_kb);
            let stat = name.map(|taskname| {
                kill_occurred(&KillStat {
                    uid: v.uid,
                    taskname: &taskname,
                    oom_score: v.adj,
                    min_oom_score: min_adj,
                    free_mem_kb: (mem.free * page_kb()) as i64,
                    rss_kb,
                })
            });
            for c in &self.connections {
                if c.events & (1 << LMK_ASYNC_EVENT_KILL) != 0 {
                    send(c.fd.as_raw_fd(), &kill);
                }
                if let Some(stat) = &stat
                    && c.events & (1 << LMK_ASYNC_EVENT_STAT) != 0
                {
                    send(c.fd.as_raw_fd(), stat);
                }
            }
            return;
        }
    }
}

fn send(fd: RawFd, bytes: &[u8]) {
    // SAFETY: a send from our buffer.
    if unsafe { libc::send(fd, bytes.as_ptr().cast(), bytes.len(), 0) } < 0 {
        log::error!(
            "control data socket write failed: {}",
            io::Error::last_os_error()
        );
    }
}

/// The guest's page size in kB.
pub fn page_kb() -> u64 {
    // SAFETY: plain sysconf.
    unsafe { libc::sysconf(libc::_SC_PAGESIZE) as u64 / 1024 }
}

struct Status {
    tgid: i32,
    rss_kb: i64,
}

/// `/proc/<pid>/status`, when there is one.
fn status(pid: i32) -> Option<Status> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let field = |name: &str| -> Option<i64> {
        let line = text.lines().find_map(|l| l.strip_prefix(name))?;
        line.split_whitespace().next()?.parse().ok()
    };
    Some(Status {
        tgid: field("Tgid:")? as i32,
        // A zombie has no VmRSS.
        rss_kb: field("VmRSS:").unwrap_or(0),
    })
}

/// The process's name: its first `cmdline` argument.
fn task_name(pid: i32) -> Option<String> {
    let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let name = cmdline.split(|&b| b == 0).next()?;
    (!name.is_empty()).then(|| String::from_utf8_lossy(name).into_owned())
}
