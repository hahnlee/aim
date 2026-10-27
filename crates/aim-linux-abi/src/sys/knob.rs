//! Kernel files whose writes act rather than store: selinuxfs's `access`
//! transaction, a thread's `comm`, a device's `uevent`.
//!
//! The fd is an unlinked file holding what a read returns, so reads, seeks
//! and fstat stay native. A write goes to the file's handler instead; a
//! reply replaces the contents and is read from the start, as a Linux
//! transaction file answers the write that precedes the read. A refused
//! write fails with the handler's errno.

use std::sync::Arc;

use super::fdtab::{self, Kind};
use crate::errno::{self, Errno};

/// The owner of the open file, independent of the process writing it.
/// Fork carries this identity, never a callback or the opener's credentials.
#[derive(Debug, PartialEq, Eq)]
pub enum Knob {
    MemfdNoexec,
    Ipv6(super::ipv6conf::Scope, String),
    SelinuxAttr(i32, String),
    SelinuxAccess,
    SelinuxContext,
    OomScoreAdj(i32),
    Comm(i32),
    Uevent(super::uevent::Device),
    FuseAbort(super::fuse::SessionKey),
}

impl Knob {
    fn request(&self, bytes: &[u8]) -> Result<Option<Vec<u8>>, Errno> {
        match self {
            Self::MemfdNoexec => super::memfd::write_noexec(bytes),
            Self::Ipv6(scope, name) => super::ipv6conf::write(*scope, name, bytes),
            Self::SelinuxAttr(opener, name) => super::selinuxfs::write_attr(*opener, name, bytes),
            Self::SelinuxAccess => Ok(Some(super::selinuxfs::access_reply())),
            // With no policy loaded every context is valid.
            Self::SelinuxContext => Ok(None),
            Self::OomScoreAdj(pid) => super::cred::write_oom_score_adj(*pid, bytes).map(|()| None),
            Self::Comm(tid) => super::procfs::write_comm(*tid, bytes).map(|()| None),
            Self::Uevent(dev) => super::uevent::synthesize(dev, bytes).map(|()| None),
            Self::FuseAbort(key) => super::fuse::abort(key).map(|()| None),
        }
    }
}

pub(super) fn save(k: &Knob, w: &mut super::fork_state::Writer) {
    match k {
        Knob::MemfdNoexec => w.u32(0),
        Knob::Ipv6(scope, name) => {
            w.u32(1);
            match scope {
                super::ipv6conf::Scope::All => w.u32(0),
                super::ipv6conf::Scope::Default => w.u32(1),
                super::ipv6conf::Scope::Dev(index) => {
                    w.u32(2);
                    w.u32(*index);
                }
            }
            w.str(name);
        }
        Knob::SelinuxAttr(opener, name) => {
            w.u32(2);
            w.i32(*opener);
            w.str(name);
        }
        Knob::SelinuxAccess => w.u32(3),
        Knob::SelinuxContext => w.u32(4),
        Knob::OomScoreAdj(pid) => {
            w.u32(5);
            w.i32(*pid);
        }
        Knob::Comm(tid) => {
            w.u32(6);
            w.i32(*tid);
        }
        Knob::Uevent(dev) => {
            w.u32(7);
            w.str(&dev.devpath);
            w.str(&dev.subsystem);
            w.seq(dev.env.iter(), |w, v| w.str(v));
        }
        Knob::FuseAbort(key) => {
            w.u32(8);
            w.path(key.transport());
        }
    }
}

pub(super) fn load(r: &mut super::fork_state::Reader) -> Option<Arc<Knob>> {
    let k = match r.u32() {
        0 => Knob::MemfdNoexec,
        1 => {
            let scope = match r.u32() {
                0 => super::ipv6conf::Scope::All,
                1 => super::ipv6conf::Scope::Default,
                2 => super::ipv6conf::Scope::Dev(r.u32()),
                _ => {
                    r.invalidate();
                    return None;
                }
            };
            Knob::Ipv6(scope, r.str())
        }
        2 => Knob::SelinuxAttr(r.i32(), r.str()),
        3 => Knob::SelinuxAccess,
        4 => Knob::SelinuxContext,
        5 => Knob::OomScoreAdj(r.i32()),
        6 => Knob::Comm(r.i32()),
        7 => Knob::Uevent(super::uevent::Device {
            devpath: r.str(),
            subsystem: r.str(),
            env: r.seq(|r| r.str()),
        }),
        8 => Knob::FuseAbort(super::fuse::SessionKey::from_transport(r.path())),
        _ => {
            r.invalidate();
            return None;
        }
    };
    r.intact().then(|| Arc::new(k))
}

/// A knob fd reading `contents`, whose writes go to its owner.
pub fn open(contents: &[u8], cloexec: bool, owner: Knob) -> i64 {
    let fd = super::procfs::content_fd_unpublished(contents, cloexec);
    if fd >= 0 {
        fdtab::insert(fd as i32, Kind::Knob(Arc::new(owner)));
        if let Err(error) = fdtab::publish_guest(fd as i32) {
            fdtab::on_close(fd as i32);
            unsafe {
                libc::close(fd as i32);
            }
            return -(error as i64);
        }
    }
    fd
}

/// write/writev on a knob: the whole write is one request.
pub fn write(fd: i32, k: &Knob, iov: &[libc::iovec]) -> i64 {
    let mut req = Vec::new();
    for v in iov {
        // SAFETY: guest buffers.
        req.extend_from_slice(unsafe {
            std::slice::from_raw_parts(v.iov_base as *const u8, v.iov_len)
        });
    }
    let reply = match k.request(&req) {
        Ok(reply) => reply,
        Err(e) => return -(e as i64),
    };
    if let Some(reply) = reply {
        // SAFETY: rewriting our own unlinked file.
        unsafe {
            libc::ftruncate(fd, 0);
            if libc::pwrite(fd, reply.as_ptr().cast(), reply.len(), 0) < 0 {
                return -(errno::last() as i64);
            }
            libc::lseek(fd, 0, libc::SEEK_SET);
        }
    }
    req.len() as i64
}

#[cfg(test)]
mod tests {
    use super::super::{
        fork_state::{Reader, Writer},
        ipv6conf::Scope,
    };
    use super::*;

    #[test]
    fn fork_preserves_every_owner_and_rejects_damage() {
        let owners = [
            Knob::MemfdNoexec,
            Knob::Ipv6(Scope::All, "forwarding".into()),
            Knob::Ipv6(Scope::Default, "hop_limit".into()),
            Knob::Ipv6(Scope::Dev(2), "stable_secret".into()),
            Knob::SelinuxAttr(43, "current".into()),
            Knob::SelinuxAccess,
            Knob::SelinuxContext,
            Knob::OomScoreAdj(43),
            Knob::Comm(44),
            Knob::Uevent(super::super::uevent::Device {
                devpath: "/devices/virtual/input/input0/event0".into(),
                subsystem: "input".into(),
                env: vec!["MAJOR=13".into(), "MINOR=64".into()],
            }),
            Knob::FuseAbort(super::super::fuse::SessionKey::from_transport(
                "/runtime/fuse/socket".into(),
            )),
        ];
        for owner in owners {
            let mut w = Writer::default();
            save(&owner, &mut w);
            let bytes = w.into_bytes();
            let mut r = Reader::new(&bytes);
            assert_eq!(*load(&mut r).unwrap(), owner);
            assert!(r.ok());
            for end in 0..bytes.len() {
                let mut r = Reader::new(&bytes[..end]);
                assert!(load(&mut r).is_none(), "{owner:?}: {end}");
                assert!(!r.intact());
            }
        }
        for tags in [[99, 0], [1, 99]] {
            let mut w = Writer::default();
            for tag in tags {
                w.u32(tag);
            }
            let bytes = w.into_bytes();
            let mut r = Reader::new(&bytes);
            assert!(load(&mut r).is_none());
            assert!(!r.intact());
        }
    }

    #[test]
    fn restored_handlers_validate_and_answer_transactions() {
        let mut w = Writer::default();
        save(&Knob::SelinuxAccess, &mut w);
        let bytes = w.into_bytes();
        let k = load(&mut Reader::new(&bytes)).unwrap();
        assert_eq!(
            k.request(b"request").unwrap().unwrap(),
            b"ffffffff ffffffff 0 ffffffff 0 0"
        );
        let mut w = Writer::default();
        save(&Knob::Comm(i32::MAX), &mut w);
        let bytes = w.into_bytes();
        let k = load(&mut Reader::new(&bytes)).unwrap();
        assert_eq!(k.request(b"name"), Err(errno::EINVAL));
        let mut w = Writer::default();
        save(&Knob::SelinuxAttr(-1, "current".into()), &mut w);
        let bytes = w.into_bytes();
        let k = load(&mut Reader::new(&bytes)).unwrap();
        assert_eq!(k.request(b"u:r:child:s0"), Err(errno::EPERM));
    }

    #[test]
    fn fork_table_restores_aliases_and_current_writer_checks() {
        const TEST: &str =
            "sys::knob::tests::fork_table_restores_aliases_and_current_writer_checks";
        const CHILD: &str = "knob-fork-table-child";
        if !std::env::args().any(|arg| arg == CHILD) {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", TEST, "--nocapture", "--skip", CHILD])
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        let _view = crate::vfs::test_view();
        let fd = open(b"", true, Knob::SelinuxAccess) as i32;
        assert!(fd >= 0);
        let alias = super::super::fs::dup([fd as u64, 0, 0, 0, 0, 0]) as i32;
        assert!(alias >= 0);
        let noexec = open(b"0\n", false, Knob::MemfdNoexec) as i32;
        assert!(noexec >= 0);
        let mut w = Writer::default();
        fdtab::fork_save(&mut w);
        // Replace the in-process kinds to prove restore reconstructs them.
        for fd in [fd, alias, noexec] {
            fdtab::insert(fd, Kind::Random);
        }
        let bytes = w.into_bytes();
        let mut r = Reader::new(&bytes);
        fdtab::fork_restore(&mut r);
        assert!(r.ok(), "{}", r.diagnostic());
        let Some(Kind::Knob(original)) = fdtab::get(fd) else {
            panic!("lost knob");
        };
        let Some(Kind::Knob(copy)) = fdtab::get(alias) else {
            panic!("lost alias");
        };
        let Some(Kind::Knob(sysctl)) = fdtab::get(noexec) else {
            panic!("lost sysctl");
        };
        assert!(Arc::ptr_eq(&original, &copy));
        assert!(!Arc::ptr_eq(&original, &sysctl));
        assert_eq!(*original, Knob::SelinuxAccess);
        assert_eq!(*sysctl, Knob::MemfdNoexec);
        assert_ne!(
            unsafe { libc::fcntl(fd, libc::F_GETFD) } & libc::FD_CLOEXEC,
            0
        );
        assert_eq!(
            unsafe { libc::fcntl(noexec, libc::F_GETFD) } & libc::FD_CLOEXEC,
            0
        );
        let req = b"request";
        let iov = libc::iovec {
            iov_base: req.as_ptr().cast_mut().cast(),
            iov_len: req.len(),
        };
        assert_eq!(write(alias, &copy, &[iov]), req.len() as i64);
        let mut reply = [0u8; 64];
        let n = unsafe { libc::read(fd, reply.as_mut_ptr().cast(), reply.len()) };
        assert_eq!(&reply[..n as usize], b"ffffffff ffffffff 0 ffffffff 0 0");
        // The deserialized owner must consult the writer, not saved credentials.
        let mut id = super::super::cred::current();
        id.cap_eff &= !(1 << super::super::cred::CAP_SYS_ADMIN);
        super::super::cred::init(id, None);
        assert_eq!(sysctl.request(b"0"), Err(errno::EPERM));
        assert_eq!(sysctl.request(b"junk"), Err(errno::EPERM));
        for fd in [fd, alias, noexec] {
            assert_eq!(super::super::fs::close([fd as u64, 0, 0, 0, 0, 0]), 0);
        }
    }
}
