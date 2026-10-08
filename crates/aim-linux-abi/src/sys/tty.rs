//! Terminals: the tty ioctls (tty_ioctl(4)) on host terminals, and Linux's
//! pseudo-terminals (pty(7)) over the host's.
//!
//! The guest's `/dev/ptmx` is the host's: opening it makes a host master,
//! whose slave the host names `/dev/ttysNNN` and the guest `/dev/pts/N`
//! ([`pts_host`], [`pts_guest`]). `TIOCGPTN` gives that number and
//! `TIOCSPTLCK` unlocks the slave (bionic's `ptsname` and `unlockpt`; its
//! `grantpt` does nothing, as devpts needs nothing), which on the host is
//! `grantpt` and `unlockpt`. Reads, writes, `poll` and the line discipline
//! are the host's. termios is translated both ways: the flags, control
//! characters and speed of Linux's `struct termios` (36 bytes on arm64)
//! against Darwin's.
//!
//! Where Darwin differs: an exiting process waits until the master has
//! read its slave output, which Linux would keep for it
//! ([`drain_on_exit`]); a master's termios exist only while its slave is
//! open (before, they are ENOTTY where Linux answers); and closing the
//! master sends the slave's session no SIGHUP (#749).

use crate::errno::{self, EINVAL, ENOTTY};
use crate::sys::pidns;

const TCGETS: u64 = 0x5401;
const TCSETS: u64 = 0x5402;
const TCSETSW: u64 = 0x5403;
const TCSETSF: u64 = 0x5404;
const TCSBRK: u64 = 0x5409;
const TCFLSH: u64 = 0x540b;
const TIOCSCTTY: u64 = 0x540e;
const TIOCGPGRP: u64 = 0x540f;
const TIOCSPGRP: u64 = 0x5410;
const TIOCGWINSZ: u64 = 0x5413;
const TIOCSWINSZ: u64 = 0x5414;
const TIOCNOTTY: u64 = 0x5422;
const TIOCGSID: u64 = 0x5429;
/// `_IOR('T', 0x30, unsigned int)`.
const TIOCGPTN: u64 = 0x8004_5430;
/// `_IOW('T', 0x31, int)`.
const TIOCSPTLCK: u64 = 0x4004_5431;
/// Darwin's `TIOCPTYGNAME`, `_IOR('t', 83, char[128])`: a master's slave.
const DARWIN_TIOCPTYGNAME: libc::c_ulong = 0x4080_7453;

/// The host's name of slave `n` (`ptsname`'s `/dev/ttys%03d`).
pub fn pts_host(guest: &str) -> Option<String> {
    let n: u32 = guest.strip_prefix("/dev/pts/")?.parse().ok()?;
    Some(format!("/dev/ttys{n:03}"))
}

/// The guest's name of host slave `host`.
pub fn pts_guest(host: &str) -> Option<String> {
    let n: u32 = host.strip_prefix("/dev/ttys")?.parse().ok()?;
    Some(format!("/dev/pts/{n}"))
}

/// Linux devpts_new_index uses the allocating filesystem identity. Pinned
/// first_stage_init mounts devpts with NULL options, whose slave mode is 0600.
/// Darwin grantpt prepares the physical node; guest attributes belong to its
/// actual allocated inode, not the host user's uid or reusable slave number.
pub(super) fn allocated_master(fd: i32, identity: &super::cred::Identity) -> Result<(), crate::errno::Errno> {
    if !super::attrs::recording() { return Ok(()); }
    let mut name = [0 as libc::c_char;128];
    if unsafe { libc::ioctl(fd, DARWIN_TIOCPTYGNAME, name.as_mut_ptr()) } != 0 { return Err(errno::last()); }
    if unsafe { libc::grantpt(fd) } != 0 { return Err(errno::last()); }
    let host = unsafe { std::ffi::CStr::from_ptr(name.as_ptr()) };
    pts_guest(host.to_str().map_err(|_| EINVAL)?).ok_or(EINVAL)?;
    super::attrs::allocated_character(super::attrs::Host::Path(host),
        super::attrs::Attr { uid:Some(identity.uid[3]), gid:Some(identity.gid[3]), mode:Some(0o600) })
}

/// The terminal ioctls; None for a request that is not one.
pub fn ioctl(fd: i32, req: u64, arg: u64) -> Option<i64> {
    if !matches!(
        req,
        TCGETS
            | TCSETS
            | TCSETSW
            | TCSETSF
            | TCSBRK
            | TCFLSH
            | TIOCSCTTY
            | TIOCGPGRP
            | TIOCSPGRP
            | TIOCGWINSZ
            | TIOCSWINSZ
            | TIOCNOTTY
            | TIOCGSID
            | TIOCGPTN
            | TIOCSPTLCK
    ) {
        return None;
    }
    // SAFETY: host terminal calls on a guest fd; `arg` is the guest's
    // buffer for the request, as the kernel would read or write it.
    Some(unsafe {
        if libc::isatty(fd) != 1 {
            return Some(-(ENOTTY as i64));
        }
        match req {
            TCGETS => {
                let mut t: libc::termios = std::mem::zeroed();
                if libc::tcgetattr(fd, &mut t) < 0 {
                    return Some(-(errno::last() as i64));
                }
                (arg as *mut [u8; LINUX_TERMIOS]).write_unaligned(to_linux(&t));
                0
            }
            TCSETS | TCSETSW | TCSETSF => {
                let mut t: libc::termios = std::mem::zeroed();
                if libc::tcgetattr(fd, &mut t) < 0 {
                    return Some(-(errno::last() as i64));
                }
                from_linux(
                    &(arg as *const [u8; LINUX_TERMIOS]).read_unaligned(),
                    &mut t,
                );
                let when = match req {
                    TCSETS => libc::TCSANOW,
                    TCSETSW => libc::TCSADRAIN,
                    _ => libc::TCSAFLUSH,
                };
                errno::check(libc::tcsetattr(fd, when, &t) as i64)
            }
            // tcdrain is TCSBRK with a non-zero argument; zero sends a break.
            TCSBRK if arg != 0 => errno::check(libc::tcdrain(fd) as i64),
            TCSBRK => errno::check(libc::tcsendbreak(fd, 0) as i64),
            TCFLSH => match arg {
                0..=2 => errno::check(libc::tcflush(
                    fd,
                    [libc::TCIFLUSH, libc::TCOFLUSH, libc::TCIOFLUSH][arg as usize],
                ) as i64),
                _ => -(EINVAL as i64),
            },
            TIOCSCTTY => {
                errno::check(libc::ioctl(fd, libc::TIOCSCTTY.into(), arg as libc::c_int) as i64)
            }
            TIOCNOTTY => errno::check(libc::ioctl(fd, libc::TIOCNOTTY) as i64),
            TIOCGPGRP => {
                let pgrp = libc::tcgetpgrp(fd);
                if pgrp < 0 {
                    return Some(-(errno::last() as i64));
                }
                (arg as *mut i32).write_unaligned(pidns::id_in_ns(pgrp));
                0
            }
            TIOCGSID => {
                let sid = libc::tcgetsid(fd);
                if sid < 0 {
                    return Some(-(errno::last() as i64));
                }
                (arg as *mut i32).write_unaligned(pidns::id_in_ns(sid));
                0
            }
            TIOCSPGRP => {
                errno::check(libc::tcsetpgrp(fd, (arg as *const i32).read_unaligned()) as i64)
            }
            TIOCGWINSZ | TIOCSWINSZ => {
                // struct winsize has the same layout on both kernels.
                let host = if req == TIOCGWINSZ {
                    libc::TIOCGWINSZ
                } else {
                    libc::TIOCSWINSZ
                };
                errno::check(libc::ioctl(fd, host, arg as *mut libc::winsize) as i64)
            }
            TIOCGPTN => {
                let mut name = [0 as libc::c_char; 128];
                if libc::ioctl(fd, DARWIN_TIOCPTYGNAME, name.as_mut_ptr()) != 0 {
                    // Not a master.
                    return Some(-(ENOTTY as i64));
                }
                let name = std::ffi::CStr::from_ptr(name.as_ptr()).to_string_lossy();
                let Some(n) =
                    pts_guest(&name).and_then(|g| g["/dev/pts/".len()..].parse::<u32>().ok())
                else {
                    return Some(-(ENOTTY as i64));
                };
                (arg as *mut u32).write_unaligned(n);
                0
            }
            // TIOCSPTLCK with 0 unlocks; the host has no way to lock again.
            _ => {
                if (arg as *const i32).read_unaligned() != 0 {
                    return Some(-(EINVAL as i64));
                }
                // Darwin's slave opens only after grantpt; devpts needs none.
                let mut name = [0 as libc::c_char;128];
                let allocated = if super::attrs::recording()
                    && libc::ioctl(fd, DARWIN_TIOCPTYGNAME, name.as_mut_ptr()) == 0 {
                    match super::attrs::character_attributes(super::attrs::Host::Path(
                        std::ffi::CStr::from_ptr(name.as_ptr()))) {
                        Ok(attributes) => attributes.is_some(),
                        Err(error) => return Some(-(error as i64)),
                    }
                } else { false };
                // Allocation already performed grantpt. Repeating it would
                // change the devfs cookie after guest ownership was recorded.
                if !allocated && libc::grantpt(fd) != 0 {
                    return Some(-(errno::last() as i64));
                }
                errno::check(libc::unlockpt(fd) as i64)
            }
        }
    })
}

/// At a process's exit: output its terminals still hold for a pty master
/// is read first. Linux keeps a slave's output for the master after the
/// slave's last close; Darwin's master reads end-of-file once its slave is
/// closed or its session leader exits, and the output is lost (an adbd
/// subprocess's, when it exits before adbd reads). So the process waits
/// until the master has read it, or closed.
pub fn drain_on_exit() {
    for fd in super::fdtab::open_fds() {
        let mut queued: libc::c_int = 0;
        // SAFETY: host terminal calls on this process's own fds.
        unsafe {
            if libc::isatty(fd) == 1
                && libc::ioctl(fd, libc::TIOCOUTQ, &mut queued) == 0
                && queued > 0
            {
                libc::tcdrain(fd);
            }
        }
    }
}

/// `sizeof(struct termios)` on arm64 Linux: four `tcflag_t`, `c_line` and
/// 19 control characters.
const LINUX_TERMIOS: usize = 36;

/// (Linux bit, Darwin bit) of each mode flag that both have.
const IFLAG: &[(u32, libc::tcflag_t)] = &[
    (0o1, libc::IGNBRK),
    (0o2, libc::BRKINT),
    (0o4, libc::IGNPAR),
    (0o10, libc::PARMRK),
    (0o20, libc::INPCK),
    (0o40, libc::ISTRIP),
    (0o100, libc::INLCR),
    (0o200, libc::IGNCR),
    (0o400, libc::ICRNL),
    (0o2000, libc::IXON),
    (0o4000, libc::IXANY),
    (0o10000, libc::IXOFF),
    (0o20000, libc::IMAXBEL),
    (0o40000, libc::IUTF8),
];
const OFLAG: &[(u32, libc::tcflag_t)] = &[
    (0o1, libc::OPOST),
    (0o4, libc::ONLCR),
    (0o10, libc::OCRNL),
    (0o20, libc::ONOCR),
    (0o40, libc::ONLRET),
    (0o100, libc::OFILL),
    (0o200, libc::OFDEL),
];
const CFLAG: &[(u32, libc::tcflag_t)] = &[
    (0o100, libc::CSTOPB),
    (0o200, libc::CREAD),
    (0o400, libc::PARENB),
    (0o1000, libc::PARODD),
    (0o2000, libc::HUPCL),
    (0o4000, libc::CLOCAL),
    (0o20000000000, libc::CRTSCTS),
];
const LFLAG: &[(u32, libc::tcflag_t)] = &[
    (0o1, libc::ISIG),
    (0o2, libc::ICANON),
    (0o10, libc::ECHO),
    (0o20, libc::ECHOE),
    (0o40, libc::ECHOK),
    (0o100, libc::ECHONL),
    (0o200, libc::NOFLSH),
    (0o400, libc::TOSTOP),
    (0o1000, libc::ECHOCTL),
    (0o2000, libc::ECHOPRT),
    (0o4000, libc::ECHOKE),
    (0o10000, libc::FLUSHO),
    (0o40000, libc::PENDIN),
    (0o100000, libc::IEXTEN),
    (0o200000, libc::EXTPROC),
];
/// Linux's `CSIZE` (`CS5` to `CS8` in steps of 0o20).
const LINUX_CSIZE: u32 = 0o60;
/// Linux's `CBAUD`: the speed's code in `c_cflag`.
const LINUX_CBAUD: u32 = 0o10017;
/// (Linux speed code, speed).
const SPEEDS: &[(u32, libc::speed_t)] = &[
    (0, 0),
    (0o1, 50),
    (0o2, 75),
    (0o3, 110),
    (0o4, 134),
    (0o5, 150),
    (0o6, 200),
    (0o7, 300),
    (0o10, 600),
    (0o11, 1200),
    (0o12, 1800),
    (0o13, 2400),
    (0o14, 4800),
    (0o15, 9600),
    (0o16, 19200),
    (0o17, 38400),
    (0o10001, 57600),
    (0o10002, 115200),
    (0o10003, 230400),
];
/// (Linux index, Darwin index) of each control character both have:
/// VINTR, VQUIT, VERASE, VKILL, VEOF, VTIME, VMIN, VSTART, VSTOP, VSUSP,
/// VEOL, VREPRINT, VDISCARD, VWERASE, VLNEXT, VEOL2.
const CC: &[(usize, usize)] = &[
    (0, libc::VINTR),
    (1, libc::VQUIT),
    (2, libc::VERASE),
    (3, libc::VKILL),
    (4, libc::VEOF),
    (5, libc::VTIME),
    (6, libc::VMIN),
    (8, libc::VSTART),
    (9, libc::VSTOP),
    (10, libc::VSUSP),
    (11, libc::VEOL),
    (12, libc::VREPRINT),
    (13, libc::VDISCARD),
    (14, libc::VWERASE),
    (15, libc::VLNEXT),
    (16, libc::VEOL2),
];

fn flags_to_linux(host: libc::tcflag_t, table: &[(u32, libc::tcflag_t)]) -> u32 {
    table
        .iter()
        .filter(|(_, h)| host & h == *h)
        .fold(0, |linux, (l, _)| linux | l)
}

/// `host` with the bits of `table` replaced by those `linux` has.
fn flags_from_linux(
    linux: u32,
    host: libc::tcflag_t,
    table: &[(u32, libc::tcflag_t)],
) -> libc::tcflag_t {
    table.iter().fold(
        host,
        |host, (l, h)| {
            if linux & l != 0 { host | h } else { host & !h }
        },
    )
}

fn to_linux(t: &libc::termios) -> [u8; LINUX_TERMIOS] {
    let size = (t.c_cflag & libc::CSIZE) >> libc::CSIZE.trailing_zeros();
    let speed = SPEEDS
        .iter()
        .find(|(_, s)| *s == t.c_ospeed)
        .map_or(0o17, |(code, _)| *code);
    let words = [
        flags_to_linux(t.c_iflag, IFLAG),
        flags_to_linux(t.c_oflag, OFLAG),
        flags_to_linux(t.c_cflag, CFLAG) | (size as u32) << 4 | speed,
        flags_to_linux(t.c_lflag, LFLAG),
    ];
    let mut out = [0u8; LINUX_TERMIOS];
    for (i, w) in words.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&w.to_ne_bytes());
    }
    // c_line (N_TTY) stays 0.
    for &(l, h) in CC {
        out[17 + l] = t.c_cc[h];
    }
    out
}

fn from_linux(linux: &[u8; LINUX_TERMIOS], t: &mut libc::termios) {
    let word = |i: usize| u32::from_ne_bytes(linux[i * 4..i * 4 + 4].try_into().unwrap());
    let cflag = word(2);
    t.c_iflag = flags_from_linux(word(0), t.c_iflag, IFLAG);
    t.c_oflag = flags_from_linux(word(1), t.c_oflag, OFLAG);
    t.c_cflag = flags_from_linux(cflag, t.c_cflag, CFLAG) & !libc::CSIZE
        | ((cflag & LINUX_CSIZE) as libc::tcflag_t >> 4) << libc::CSIZE.trailing_zeros();
    t.c_lflag = flags_from_linux(word(3), t.c_lflag, LFLAG);
    if let Some((_, speed)) = SPEEDS.iter().find(|(code, _)| *code == cflag & LINUX_CBAUD) {
        t.c_ispeed = *speed;
        t.c_ospeed = *speed;
    }
    for &(l, h) in CC {
        t.c_cc[h] = linux[17 + l];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pts_names_round_trip() {
        assert_eq!(pts_host("/dev/pts/7").as_deref(), Some("/dev/ttys007"));
        assert_eq!(pts_host("/dev/pts/1234").as_deref(), Some("/dev/ttys1234"));
        assert_eq!(pts_guest("/dev/ttys007").as_deref(), Some("/dev/pts/7"));
        assert_eq!(pts_host("/dev/pts/x"), None);
        assert_eq!(pts_host("/dev/ptmx"), None);
        assert_eq!(pts_guest("/dev/ttyp0"), None);
    }

    #[test]
    fn termios_round_trips_through_linux() {
        // SAFETY: plain struct.
        let mut t: libc::termios = unsafe { std::mem::zeroed() };
        t.c_iflag = libc::ICRNL | libc::IXON | libc::IUTF8;
        t.c_oflag = libc::OPOST | libc::ONLCR;
        t.c_cflag = libc::CS8 | libc::CREAD | libc::HUPCL;
        t.c_lflag = libc::ISIG | libc::ICANON | libc::ECHO | libc::ECHOE | libc::IEXTEN;
        t.c_ispeed = 38400;
        t.c_ospeed = 38400;
        t.c_cc[libc::VINTR] = 3;
        t.c_cc[libc::VMIN] = 1;
        let linux = to_linux(&t);
        let word = |i: usize| u32::from_ne_bytes(linux[i * 4..i * 4 + 4].try_into().unwrap());
        // Linux's values (asm-generic/termbits.h).
        assert_eq!(word(0), 0o400 | 0o2000 | 0o40000);
        assert_eq!(word(1), 0o1 | 0o4);
        assert_eq!(word(2), 0o60 | 0o200 | 0o2000 | 0o17);
        assert_eq!(word(3), 0o1 | 0o2 | 0o10 | 0o20 | 0o100000);
        assert_eq!(linux[17], 3);
        assert_eq!(linux[17 + 6], 1);
        // SAFETY: plain struct.
        let mut back: libc::termios = unsafe { std::mem::zeroed() };
        from_linux(&linux, &mut back);
        assert_eq!(
            (back.c_iflag, back.c_oflag, back.c_cflag, back.c_lflag),
            (t.c_iflag, t.c_oflag, t.c_cflag, t.c_lflag)
        );
        assert_eq!(back.c_ospeed, 38400);
        assert_eq!(back.c_cc[libc::VINTR], 3);
    }

    #[test]
    fn pty_allocator_filesystem_identity_survives_unlock_and_reuse_without_foreign_grants() {
        use std::{ffi::CString, os::fd::{OwnedFd, FromRawFd, AsRawFd}};
        use crate::sys::{attrs, fs, fsops};
        let (_guard, root) = crate::vfs::test_view();
        let device = root.join("shell-pty-device");
        std::fs::create_dir_all(device.join("pts")).unwrap();
        crate::vfs::add_mount("/dev", device.clone(), crate::vfs::Area::Writable, "shell-pty-device", "tmpfs");
        let mut shell = crate::sys::cred::current();
        shell.uid = [2000;4]; shell.gid = [2000;4]; shell.groups.clear();
        shell.cap_eff = 0; shell.cap_perm = 0;
        let mut foreign = shell.clone(); foreign.uid = [2001;4]; foreign.gid = [2001;4];
        let allocate = |identity: &crate::sys::cred::Identity| unsafe {
            let fd = fs::openat_as([crate::vfs::LINUX_AT_FDCWD as u64, c"/dev/ptmx".as_ptr() as u64, 2 | 0x100, 0, 0, 0],identity);
            assert!(fd>=0,"PTY allocation: {fd}");
            let master = OwnedFd::from_raw_fd(fd as i32);
            let mut number = u32::MAX;
            assert_eq!(ioctl(master.as_raw_fd(),TIOCGPTN,&mut number as *mut u32 as u64),Some(0));
            (master,number)
        };
        let unlock = |master: &OwnedFd| { let mut value=0i32;
            assert_eq!(ioctl(master.as_raw_fd(),TIOCSPTLCK,&mut value as *mut i32 as u64),Some(0)); };
        let open = |path: &CString, identity: &crate::sys::cred::Identity| fs::openat_as(
            [crate::vfs::LINUX_AT_FDCWD as u64,path.as_ptr() as u64,2|0x100,0,0,0],identity);
        let (master,number) = allocate(&shell);
        let path = CString::new(format!("/dev/pts/{number}")).unwrap();
        let stat = attrs::path_stat(path.to_str().unwrap()).unwrap();
        assert_eq!((stat.st_uid,stat.st_gid,stat.st_mode & 0o777),(2000,2000,0o600));
        unlock(&master);
        assert_eq!(fsops::fchmodat([crate::vfs::LINUX_AT_FDCWD as u64,path.as_ptr() as u64,0o640,0,0,0]),0);
        assert_eq!(fsops::fchownat([crate::vfs::LINUX_AT_FDCWD as u64,path.as_ptr() as u64,2000,2020,0,0]),0);
        // Physical backend grant operations may alter ctime. Guest ownership
        // is bound to the immutable allocation inode/generation, not ctime.
        assert_eq!(unsafe{libc::grantpt(master.as_raw_fd())},0);
        assert_eq!(unsafe{libc::grantpt(master.as_raw_fd())},0);
        unlock(&master);
        let changed = attrs::path_stat(path.to_str().unwrap()).unwrap();
        assert_eq!((changed.st_uid,changed.st_gid,changed.st_mode & 0o777),(2000,2020,0o640));
        assert_eq!(open(&path,&foreign),-(crate::errno::EACCES as i64));
        let slave = open(&path,&shell); assert!(slave>=0);
        assert_eq!(fs::close([slave as u64,0,0,0,0,0]),0); drop(master);
        let mut held = Vec::new(); let mut reused = None;
        // Retain lower-numbered allocations while searching so unrelated host
        // PTYs cannot make an immediate-global-reuse assumption flaky.
        for _ in 0..64 {
            let allocation = allocate(&foreign);
            if allocation.1 == number { reused = Some(allocation.0); break; }
            held.push(allocation.0);
        }
        let master = reused.expect("freed owned slave not found within allocation bound");
        unlock(&master);
        let fresh = attrs::path_stat(path.to_str().unwrap()).unwrap();
        assert_ne!((fresh.st_dev,fresh.st_ino),(stat.st_dev,stat.st_ino));
        assert_eq!((fresh.st_uid,fresh.st_gid,fresh.st_mode & 0o777),(2001,2001,0o600));
        assert_eq!(open(&path,&shell),-(crate::errno::EACCES as i64));
        let slave = open(&path,&foreign); assert!(slave>=0);
        assert_eq!(fs::close([slave as u64,0,0,0,0,0]),0); drop(master); drop(held);
        // A durable metadata failure must fail allocation and close its new
        // master, rather than report an unowned successful terminal.
        let journal = crate::vfs::runtime_dir().unwrap().join("fs-attrs");
        let saved = journal.with_extension("pty-test-saved");
        std::fs::rename(&journal,&saved).unwrap();
        std::fs::create_dir(&journal).unwrap();
        let failed = fs::openat_as([crate::vfs::LINUX_AT_FDCWD as u64,c"/dev/ptmx".as_ptr() as u64,2|0x100,0,0,0],&shell);
        std::fs::remove_dir(&journal).unwrap();
        std::fs::rename(saved,journal).unwrap();
        assert_eq!(failed,-(crate::errno::EISDIR as i64));
        assert!(crate::vfs::remove_mount("/dev").unwrap());
        std::fs::remove_dir_all(device).unwrap();
    }

    #[test]
    fn pty_slave_open_obeys_real_guest_namespace_ancestors_and_exec_import() {
        use std::ffi::CString;
        let (_guard, root) = crate::vfs::test_view();
        let device = root.join("owned-pty-dev");
        std::fs::create_dir_all(&device).unwrap();
        crate::vfs::add_mount("/dev", device.clone(), crate::vfs::Area::Writable, "owned-pty-dev", "tmpfs");
        unsafe {
            let master = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY);
            assert!(master >= 0);
            let mut unlock = 0i32;
            assert_eq!(ioctl(master, TIOCSPTLCK, &mut unlock as *mut i32 as u64), Some(0));
            let mut number = u32::MAX;
            assert_eq!(ioctl(master, TIOCGPTN, &mut number as *mut u32 as u64), Some(0));
            let slave = CString::new(format!("/dev/pts/{number}")).unwrap();
            let open = || crate::sys::fs::openat([crate::vfs::LINUX_AT_FDCWD as u64, slave.as_ptr() as u64, 2 | 0x100, 0, 0, 0]);
            // A real master does not make a missing guest ancestor searchable.
            assert_eq!(open(), -(crate::errno::ENOENT as i64));
            std::fs::create_dir(device.join("pts")).unwrap();
            for imported in [false, true] {
                if imported { let mounts = crate::vfs::own_mounts_text(); crate::vfs::load_own_mounts(&mounts).unwrap(); }
                let fd = open(); assert!(fd >= 0, "PTY slave open after import={imported}: {fd}");
                let mut size: libc::winsize = std::mem::zeroed();
                assert_eq!(libc::ioctl(fd as i32, libc::TIOCGWINSZ, &mut size), 0);
                assert_eq!(crate::sys::fs::close([fd as u64, 0, 0, 0, 0, 0]), 0);
            }
            libc::close(master);
        }
        assert!(crate::vfs::remove_mount("/dev").unwrap());
        // The same-process import adds a second owned entry; the real exec
        // starts with only map entries, then imports its one inherited mount.
        assert!(crate::vfs::remove_mount("/dev").unwrap());
        std::fs::remove_dir_all(device).unwrap();
    }

    #[test]
    fn a_pty_pair_through_the_ioctls() {
        // SAFETY: a host pty pair made and closed here.
        unsafe {
            let master = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY);
            assert!(master >= 0);
            let mut unlock = 0i32;
            assert_eq!(
                ioctl(master, TIOCSPTLCK, &mut unlock as *mut i32 as u64),
                Some(0)
            );
            let mut n = u32::MAX;
            assert_eq!(ioctl(master, TIOCGPTN, &mut n as *mut u32 as u64), Some(0));
            let host = pts_host(&format!("/dev/pts/{n}")).unwrap();
            let slave = libc::open(
                std::ffi::CString::new(host).unwrap().as_ptr(),
                libc::O_RDWR | libc::O_NOCTTY,
            );
            assert!(slave >= 0);
            // cfmakeraw, as adbd makes a raw pty, through Linux's termios.
            let mut linux = [0u8; LINUX_TERMIOS];
            assert_eq!(ioctl(slave, TCGETS, linux.as_mut_ptr() as u64), Some(0));
            let lflag = u32::from_ne_bytes(linux[12..16].try_into().unwrap());
            linux[12..16].copy_from_slice(&(lflag & !(0o2 | 0o10 | 0o1 | 0o100000)).to_ne_bytes());
            let oflag = u32::from_ne_bytes(linux[4..8].try_into().unwrap());
            linux[4..8].copy_from_slice(&(oflag & !0o1).to_ne_bytes());
            assert_eq!(ioctl(slave, TCSETSW, linux.as_ptr() as u64), Some(0));
            let mut t: libc::termios = std::mem::zeroed();
            assert_eq!(libc::tcgetattr(slave, &mut t), 0);
            assert_eq!(t.c_lflag & (libc::ICANON | libc::ECHO), 0);
            // Raw: a newline goes through as is.
            assert_eq!(libc::write(slave, b"a\nb".as_ptr().cast(), 3), 3);
            let mut buf = [0u8; 8];
            let got = libc::read(master, buf.as_mut_ptr().cast(), buf.len());
            assert_eq!(&buf[..got as usize], b"a\nb");
            // Not a terminal.
            let mut fds = [0; 2];
            assert_eq!(libc::pipe(fds.as_mut_ptr()), 0);
            assert_eq!(
                ioctl(fds[0], TCGETS, linux.as_mut_ptr() as u64),
                Some(-(ENOTTY as i64))
            );
            for fd in [slave, master, fds[0], fds[1]] {
                libc::close(fd);
            }
        }
    }
}
