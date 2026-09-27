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

use darwin_binder_driver::Errno;

pub const OPEN: i32 = 0x6264_0001;
pub const THREAD: i32 = 0x6264_0002;
pub const MMAP: i32 = 0x6264_0003;
pub const POLL: i32 = 0x6264_0004;
pub const IOCTL: i32 = 0x6264_0005;
pub const INTERRUPT: i32 = 0x6264_0006;
pub const REPLY: i32 = 0x6264_0100;

/// Linux EPROTO: a malformed message.
pub const EPROTO: Errno = 71;

#[derive(Default)]
pub struct Writer(pub Vec<u8>);

impl Writer {
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
    /// Placeholder fds the daemon may hand out for received files.
    pub reserved: Vec<u32>,
}

impl Ioctl {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::default();
        w.u32(self.cmd).bytes(&self.arg);
        w.u32(self.segments.len() as u32);
        for (addr, bytes) in &self.segments {
            w.u64(*addr).bytes(bytes);
        }
        w.u32(self.fds.len() as u32);
        for fd in &self.fds {
            w.u32(*fd);
        }
        w.u32(self.reserved.len() as u32);
        for fd in &self.reserved {
            w.u32(*fd);
        }
        w.0
    }

    pub fn decode(b: &[u8]) -> Result<Self, Errno> {
        let mut r = Reader::new(b);
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
        }
        for _ in 0..r.u32()? {
            io.reserved.push(r.u32()?);
        }
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
    /// Fds of freed fd arrays to close.
    pub closes: Vec<u32>,
    /// Readiness bytes to drain from the binder fd.
    pub drain: u32,
}

impl IoctlReply {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::default();
        w.i32(self.status).bytes(&self.arg);
        w.u32(self.writes.len() as u32);
        for (addr, bytes) in &self.writes {
            w.u64(*addr).bytes(bytes);
        }
        w.u32(self.installs.len() as u32);
        for fd in &self.installs {
            w.u32(*fd);
        }
        w.u32(self.closes.len() as u32);
        for fd in &self.closes {
            w.u32(*fd);
        }
        w.u32(self.drain);
        w.0
    }

    pub fn decode(b: &[u8]) -> Result<Self, Errno> {
        let mut r = Reader::new(b);
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
        }
        for _ in 0..r.u32()? {
            rep.closes.push(r.u32()?);
        }
        rep.drain = r.u32()?;
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
            reserved: vec![9],
        };
        assert_eq!(Ioctl::decode(&io.encode()).unwrap(), io);
        let rep = IoctlReply {
            status: 4,
            arg: vec![5; 4],
            writes: vec![(0x3000, vec![6; 8])],
            installs: vec![9],
            closes: vec![7],
            drain: 1,
        };
        assert_eq!(IoctlReply::decode(&rep.encode()).unwrap(), rep);
        assert_eq!(Ioctl::decode(&[1, 2]), Err(EPROTO));
    }
}
