//! Requests between a guest's syscall layer and the daemon, and their
//! payload encoding (little-endian, length-prefixed byte strings).
//!
//! | id | sent to | request | reply |
//! | --- | --- | --- | --- |
//! | [`OPEN`] | service port | device, O_NONBLOCK, euid, security context; the readiness socket as a fileport | status; the file port |
//! | [`THREAD`] | file port | tid | status; the thread port |
//! | [`MMAP`] | file port | guest address, length | status; the memory entry |
//! | [`POLL`] | file port | tid | status, readiness drain |
//! | [`INTERRUPT`] | file port | tid | status |
//! | [`IOCTL`] | thread port | [`Ioctl`] | [`IoctlReply`] |
//! | [`FILES`] | service port | index of the first file | status, the number of files, then each file's device, inode and size from that index on; their memory entries |
//! | [`FILE_CLASS`] | service port | actual descriptor as a fileport | status, explicitly registered capability class |

use aim_binder_driver::Errno;

pub const OPEN: i32 = 0x6264_0001;
pub const THREAD: i32 = 0x6264_0002;
pub const MMAP: i32 = 0x6264_0003;
pub const POLL: i32 = 0x6264_0004;
pub const IOCTL: i32 = 0x6264_0008;
const IOCTL_VERSION: u32 = 2;
pub const INTERRUPT: i32 = 0x6264_0006;
pub const FILES: i32 = 0x6264_0007;
pub const FILE_CLASS: i32 = 0x6264_0009;
pub const REPLY: i32 = 0x6264_0100;

/// A file whose pages the daemon's process shares as a memory object
/// ([`crate::server::Server::share_file`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SharedFile {
    pub dev: u64,
    pub ino: u64,
    pub size: u64,
    /// A read-only memory entry of the file's pages (a send right).
    pub entry: crate::mach::Port,
}

/// Linux EPROTO: a malformed message.
pub const EPROTO: Errno = 71;

#[derive(Default)]
pub struct Writer(pub Vec<u8>);

impl Writer {
    pub fn with_capacity(n: usize) -> Self {
        Self(Vec::with_capacity(n))
    }

    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }

    pub fn i32(&mut self, v: i32) -> &mut Self {
        self.u32(v as u32)
    }

    pub fn u64(&mut self, v: u64) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }

    pub fn bytes(&mut self, b: &[u8]) -> &mut Self {
        self.u32(b.len() as u32);
        self.0.extend_from_slice(b);
        self
    }
}

pub struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    pub fn new(b: &'a [u8]) -> Self {
        Self(b)
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], Errno> {
        if self.0.len() < n {
            return Err(EPROTO);
        }
        let (head, tail) = self.0.split_at(n);
        self.0 = tail;
        Ok(head)
    }

    pub fn u32(&mut self) -> Result<u32, Errno> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    pub fn i32(&mut self) -> Result<i32, Errno> {
        Ok(self.u32()? as i32)
    }

    pub fn u64(&mut self) -> Result<u64, Errno> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    pub fn bytes(&mut self) -> Result<&'a [u8], Errno> {
        let n = self.u32()? as usize;
        self.take(n)
    }
}

/// One binder ioctl. The syscall layer copied the argument structure in and
/// gathered the guest memory the driver will read: the write stream, and
/// each transaction's data, offsets and scatter-gather buffers.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Ioctl {
    pub cmd: u32,
    pub arg: Vec<u8>,
    /// (guest address, bytes).
    pub segments: Vec<(u64, Vec<u8>)>,
    /// Guest fds of FD and FDA objects; their fileports ride along as the
    /// message's ports, in the same order.
    pub fds: Vec<u32>,
    pub file_classes: Vec<u32>,
    /// Placeholder fds the daemon may hand out for received files.
    pub reserved: Vec<u32>,
    /// The caller can prepare more placeholders when a transaction needs
    /// them ([`IoctlReply::want_fds`]); otherwise the transaction's files
    /// fail to install, with `EMFILE`.
    pub grow: bool,
}

impl Ioctl {
    pub fn encode(&self) -> Vec<u8> {
        let segments: usize = self.segments.iter().map(|(_, b)| 12 + b.len()).sum();
        let fds = 4 * (self.fds.len() + self.reserved.len());
        let mut w = Writer::with_capacity(28 + self.arg.len() + segments + fds);
        w.u32(IOCTL_VERSION).u32(self.cmd).bytes(&self.arg);
        w.u32(self.segments.len() as u32);
        for (addr, bytes) in &self.segments {
            w.u64(*addr).bytes(bytes);
        }
        w.u32(self.fds.len() as u32);
        for (index, fd) in self.fds.iter().enumerate() {
            w.u32(*fd)
                .u32(self.file_classes.get(index).copied().unwrap_or(0));
        }
        w.u32(self.reserved.len() as u32);
        for fd in &self.reserved {
            w.u32(*fd);
        }
        w.u32(self.grow as u32);
        w.0
    }

    pub fn decode(b: &[u8]) -> Result<Self, Errno> {
        let mut r = Reader::new(b);
        if r.u32()? != IOCTL_VERSION {
            return Err(EPROTO);
        }
        let mut io = Ioctl {
            cmd: r.u32()?,
            arg: r.bytes()?.to_vec(),
            ..Default::default()
        };
        for _ in 0..r.u32()? {
            io.segments.push((r.u64()?, r.bytes()?.to_vec()));
        }
        for _ in 0..r.u32()? {
            io.fds.push(r.u32()?);
            let class = r.u32()?;
            if class > crate::proxy_file::CLASS {
                return Err(EPROTO);
            }
            io.file_classes.push(class);
        }
        for _ in 0..r.u32()? {
            io.reserved.push(r.u32()?);
        }
        io.grow = r.u32()? != 0;
        Ok(io)
    }
}

/// What the syscall layer applies when an ioctl returns.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct IoctlReply {
    /// 0 or a positive Linux errno.
    pub status: i32,
    pub arg: Vec<u8>,
    /// copy_to_user, in order.
    pub writes: Vec<(u64, Vec<u8>)>,
    /// Reserved fds to replace with received files (the message's ports,
    /// in the same order).
    pub installs: Vec<u32>,
    pub file_classes: Vec<u32>,
    /// Fds of freed fd arrays to close.
    pub closes: Vec<u32>,
    /// Readiness bytes to drain from the binder fd.
    pub drain: u32,
    /// A transaction carries more files than `reserved` had room for: the
    /// read stopped before it, and the caller reads again with this many.
    pub want_fds: u32,
}

impl IoctlReply {
    pub fn encode(&self) -> Vec<u8> {
        let writes: usize = self.writes.iter().map(|(_, b)| 12 + b.len()).sum();
        let fds = 4 * (self.installs.len() + self.closes.len());
        let mut w = Writer::with_capacity(28 + self.arg.len() + writes + fds);
        w.u32(IOCTL_VERSION).i32(self.status).bytes(&self.arg);
        w.u32(self.writes.len() as u32);
        for (addr, bytes) in &self.writes {
            w.u64(*addr).bytes(bytes);
        }
        w.u32(self.installs.len() as u32);
        for (index, fd) in self.installs.iter().enumerate() {
            w.u32(*fd)
                .u32(self.file_classes.get(index).copied().unwrap_or(0));
        }
        w.u32(self.closes.len() as u32);
        for fd in &self.closes {
            w.u32(*fd);
        }
        w.u32(self.drain).u32(self.want_fds);
        w.0
    }

    pub fn decode(b: &[u8]) -> Result<Self, Errno> {
        let mut r = Reader::new(b);
        if r.u32()? != IOCTL_VERSION {
            return Err(EPROTO);
        }
        let mut rep = IoctlReply {
            status: r.i32()?,
            arg: r.bytes()?.to_vec(),
            ..Default::default()
        };
        for _ in 0..r.u32()? {
            rep.writes.push((r.u64()?, r.bytes()?.to_vec()));
        }
        for _ in 0..r.u32()? {
            rep.installs.push(r.u32()?);
            let class = r.u32()?;
            if class > crate::proxy_file::CLASS {
                return Err(EPROTO);
            }
            rep.file_classes.push(class);
        }
        for _ in 0..r.u32()? {
            rep.closes.push(r.u32()?);
        }
        rep.drain = r.u32()?;
        rep.want_fds = r.u32()?;
        Ok(rep)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ioctl_round_trips() {
        let io = Ioctl {
            cmd: 0xc030_6201,
            arg: vec![1; 48],
            segments: vec![(0x1000, vec![2; 12]), (0x2000, vec![])],
            fds: vec![3, 4],
            file_classes: vec![0, 0],
            reserved: vec![9],
            grow: true,
        };
        assert_eq!(Ioctl::decode(&io.encode()).unwrap(), io);
        let rep = IoctlReply {
            status: 4,
            arg: vec![5; 4],
            writes: vec![(0x3000, vec![6; 8])],
            installs: vec![9],
            file_classes: vec![0],
            closes: vec![7],
            drain: 1,
            want_fds: 12,
        };
        assert_eq!(IoctlReply::decode(&rep.encode()).unwrap(), rep);
        assert_eq!(Ioctl::decode(&[1, 2]), Err(EPROTO));
    }
    #[test]
    fn typed_capability_classes_are_versioned_and_unknown_classes_rejected() {
        let mut reply = IoctlReply {
            installs: vec![7],
            file_classes: vec![crate::proxy_file::CLASS],
            ..Default::default()
        };
        assert_eq!(IoctlReply::decode(&reply.encode()).unwrap(), reply);
        reply.file_classes[0] = 99;
        assert_eq!(IoctlReply::decode(&reply.encode()), Err(EPROTO));
        let mut legacy = reply.encode();
        legacy[..4].copy_from_slice(&1u32.to_le_bytes());
        assert_eq!(IoctlReply::decode(&legacy), Err(EPROTO));
    }
}
