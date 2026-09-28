//! The 64-bit Linux binder UAPI (`include/uapi/linux/android/binder.h`,
//! protocol version 8): ioctl, command and return numbers, and the byte
//! layouts of the structures that cross the ioctl boundary.
//!
//! Everything is little-endian arm64 layout. Structures are read from and
//! written to byte slices at explicit offsets; no guest structure is ever
//! reinterpreted in place.

const IOC_NONE: u32 = 0;
const IOC_WRITE: u32 = 1;
const IOC_READ: u32 = 2;

const fn ioc(dir: u32, ty: u8, nr: u32, size: usize) -> u32 {
    (dir << 30) | ((size as u32) << 16) | ((ty as u32) << 8) | nr
}
const fn io(ty: u8, nr: u32) -> u32 {
    ioc(IOC_NONE, ty, nr, 0)
}
const fn iow(ty: u8, nr: u32, size: usize) -> u32 {
    ioc(IOC_WRITE, ty, nr, size)
}
const fn ior(ty: u8, nr: u32, size: usize) -> u32 {
    ioc(IOC_READ, ty, nr, size)
}
const fn iowr(ty: u8, nr: u32, size: usize) -> u32 {
    ioc(IOC_READ | IOC_WRITE, ty, nr, size)
}

/// `_IOC_SIZE(cmd)`: the byte size of an ioctl's argument structure.
pub const fn ioc_size(cmd: u32) -> usize {
    ((cmd >> 16) & 0x3fff) as usize
}

pub const CURRENT_PROTOCOL_VERSION: i32 = 8;

// Structure sizes.
pub const WRITE_READ_SIZE: usize = 48;
pub const TRANSACTION_DATA_SIZE: usize = 64;
pub const TRANSACTION_DATA_SECCTX_SIZE: usize = 72;
pub const TRANSACTION_DATA_SG_SIZE: usize = 72;
pub const FLAT_BINDER_OBJECT_SIZE: usize = 24;
pub const FD_OBJECT_SIZE: usize = 24;
pub const BUFFER_OBJECT_SIZE: usize = 40;
pub const FD_ARRAY_OBJECT_SIZE: usize = 32;
pub const PTR_COOKIE_SIZE: usize = 16;
/// `struct binder_handle_cookie` is `__packed`.
pub const HANDLE_COOKIE_SIZE: usize = 12;
pub const NODE_DEBUG_INFO_SIZE: usize = 24;
pub const NODE_INFO_FOR_REF_SIZE: usize = 24;
pub const FREEZE_INFO_SIZE: usize = 12;
pub const FROZEN_STATUS_INFO_SIZE: usize = 12;
pub const EXTENDED_ERROR_SIZE: usize = 12;

// ioctls.
pub const BINDER_WRITE_READ: u32 = iowr(b'b', 1, WRITE_READ_SIZE);
pub const BINDER_SET_IDLE_TIMEOUT: u32 = iow(b'b', 3, 8);
pub const BINDER_SET_MAX_THREADS: u32 = iow(b'b', 5, 4);
pub const BINDER_SET_IDLE_PRIORITY: u32 = iow(b'b', 6, 4);
pub const BINDER_SET_CONTEXT_MGR: u32 = iow(b'b', 7, 4);
pub const BINDER_THREAD_EXIT: u32 = iow(b'b', 8, 4);
pub const BINDER_VERSION: u32 = iowr(b'b', 9, 4);
pub const BINDER_GET_NODE_DEBUG_INFO: u32 = iowr(b'b', 11, NODE_DEBUG_INFO_SIZE);
pub const BINDER_GET_NODE_INFO_FOR_REF: u32 = iowr(b'b', 12, NODE_INFO_FOR_REF_SIZE);
pub const BINDER_SET_CONTEXT_MGR_EXT: u32 = iow(b'b', 13, FLAT_BINDER_OBJECT_SIZE);
pub const BINDER_FREEZE: u32 = iow(b'b', 14, FREEZE_INFO_SIZE);
pub const BINDER_GET_FROZEN_INFO: u32 = iowr(b'b', 15, FROZEN_STATUS_INFO_SIZE);
pub const BINDER_ENABLE_ONEWAY_SPAM_DETECTION: u32 = iow(b'b', 16, 4);
pub const BINDER_GET_EXTENDED_ERROR: u32 = iowr(b'b', 17, EXTENDED_ERROR_SIZE);

// Object types.
const fn pack_chars(c1: u8, c2: u8, c3: u8, c4: u8) -> u32 {
    ((c1 as u32) << 24) | ((c2 as u32) << 16) | ((c3 as u32) << 8) | c4 as u32
}
const TYPE_LARGE: u8 = 0x85;
pub const BINDER_TYPE_BINDER: u32 = pack_chars(b's', b'b', b'*', TYPE_LARGE);
pub const BINDER_TYPE_WEAK_BINDER: u32 = pack_chars(b'w', b'b', b'*', TYPE_LARGE);
pub const BINDER_TYPE_HANDLE: u32 = pack_chars(b's', b'h', b'*', TYPE_LARGE);
pub const BINDER_TYPE_WEAK_HANDLE: u32 = pack_chars(b'w', b'h', b'*', TYPE_LARGE);
pub const BINDER_TYPE_FD: u32 = pack_chars(b'f', b'd', b'*', TYPE_LARGE);
pub const BINDER_TYPE_FDA: u32 = pack_chars(b'f', b'd', b'a', TYPE_LARGE);
pub const BINDER_TYPE_PTR: u32 = pack_chars(b'p', b't', b'*', TYPE_LARGE);

// flat_binder_object flags.
pub const FLAT_BINDER_FLAG_PRIORITY_MASK: u32 = 0xff;
pub const FLAT_BINDER_FLAG_ACCEPTS_FDS: u32 = 0x100;
pub const FLAT_BINDER_FLAG_SCHED_POLICY_MASK: u32 = 3 << 9;
pub const FLAT_BINDER_FLAG_INHERIT_RT: u32 = 0x800;
pub const FLAT_BINDER_FLAG_TXN_SECURITY_CTX: u32 = 0x1000;

pub const BINDER_BUFFER_FLAG_HAS_PARENT: u32 = 0x01;

// Transaction flags.
pub const TF_ONE_WAY: u32 = 0x01;
pub const TF_ROOT_OBJECT: u32 = 0x04;
pub const TF_STATUS_CODE: u32 = 0x08;
pub const TF_ACCEPT_FDS: u32 = 0x10;
pub const TF_CLEAR_BUF: u32 = 0x20;
pub const TF_UPDATE_TXN: u32 = 0x40;

// Return protocol (driver to user space).
pub const BR_ERROR: u32 = ior(b'r', 0, 4);
pub const BR_OK: u32 = io(b'r', 1);
pub const BR_TRANSACTION_SEC_CTX: u32 = ior(b'r', 2, TRANSACTION_DATA_SECCTX_SIZE);
pub const BR_TRANSACTION: u32 = ior(b'r', 2, TRANSACTION_DATA_SIZE);
pub const BR_REPLY: u32 = ior(b'r', 3, TRANSACTION_DATA_SIZE);
pub const BR_ACQUIRE_RESULT: u32 = ior(b'r', 4, 4);
pub const BR_DEAD_REPLY: u32 = io(b'r', 5);
pub const BR_TRANSACTION_COMPLETE: u32 = io(b'r', 6);
pub const BR_INCREFS: u32 = ior(b'r', 7, PTR_COOKIE_SIZE);
pub const BR_ACQUIRE: u32 = ior(b'r', 8, PTR_COOKIE_SIZE);
pub const BR_RELEASE: u32 = ior(b'r', 9, PTR_COOKIE_SIZE);
pub const BR_DECREFS: u32 = ior(b'r', 10, PTR_COOKIE_SIZE);
pub const BR_ATTEMPT_ACQUIRE: u32 = ior(b'r', 11, 24);
pub const BR_NOOP: u32 = io(b'r', 12);
pub const BR_SPAWN_LOOPER: u32 = io(b'r', 13);
pub const BR_FINISHED: u32 = io(b'r', 14);
pub const BR_DEAD_BINDER: u32 = ior(b'r', 15, 8);
pub const BR_CLEAR_DEATH_NOTIFICATION_DONE: u32 = ior(b'r', 16, 8);
pub const BR_FAILED_REPLY: u32 = io(b'r', 17);
pub const BR_FROZEN_REPLY: u32 = io(b'r', 18);
pub const BR_ONEWAY_SPAM_SUSPECT: u32 = io(b'r', 19);
pub const BR_TRANSACTION_PENDING_FROZEN: u32 = io(b'r', 20);
pub const BR_FROZEN_BINDER: u32 = ior(b'r', 21, 16);
pub const BR_CLEAR_FREEZE_NOTIFICATION_DONE: u32 = ior(b'r', 22, 8);

// Command protocol (user space to driver).
pub const BC_TRANSACTION: u32 = iow(b'c', 0, TRANSACTION_DATA_SIZE);
pub const BC_REPLY: u32 = iow(b'c', 1, TRANSACTION_DATA_SIZE);
pub const BC_ACQUIRE_RESULT: u32 = iow(b'c', 2, 4);
pub const BC_FREE_BUFFER: u32 = iow(b'c', 3, 8);
pub const BC_INCREFS: u32 = iow(b'c', 4, 4);
pub const BC_ACQUIRE: u32 = iow(b'c', 5, 4);
pub const BC_RELEASE: u32 = iow(b'c', 6, 4);
pub const BC_DECREFS: u32 = iow(b'c', 7, 4);
pub const BC_INCREFS_DONE: u32 = iow(b'c', 8, PTR_COOKIE_SIZE);
pub const BC_ACQUIRE_DONE: u32 = iow(b'c', 9, PTR_COOKIE_SIZE);
pub const BC_ATTEMPT_ACQUIRE: u32 = iow(b'c', 10, 8);
pub const BC_REGISTER_LOOPER: u32 = io(b'c', 11);
pub const BC_ENTER_LOOPER: u32 = io(b'c', 12);
pub const BC_EXIT_LOOPER: u32 = io(b'c', 13);
pub const BC_REQUEST_DEATH_NOTIFICATION: u32 = iow(b'c', 14, HANDLE_COOKIE_SIZE);
pub const BC_CLEAR_DEATH_NOTIFICATION: u32 = iow(b'c', 15, HANDLE_COOKIE_SIZE);
pub const BC_DEAD_BINDER_DONE: u32 = iow(b'c', 16, 8);
pub const BC_TRANSACTION_SG: u32 = iow(b'c', 17, TRANSACTION_DATA_SG_SIZE);
pub const BC_REPLY_SG: u32 = iow(b'c', 18, TRANSACTION_DATA_SG_SIZE);
pub const BC_REQUEST_FREEZE_NOTIFICATION: u32 = iow(b'c', 19, HANDLE_COOKIE_SIZE);
pub const BC_CLEAR_FREEZE_NOTIFICATION: u32 = iow(b'c', 20, HANDLE_COOKIE_SIZE);
pub const BC_FREEZE_NOTIFICATION_DONE: u32 = iow(b'c', 21, 8);

/// The payload size of a BC command word, or `None` for an unknown command.
/// The size is taken from the command word itself, as `_IOC_SIZE`, but only
/// for commands the protocol defines.
pub fn command_payload_size(cmd: u32) -> Option<usize> {
    match cmd {
        BC_TRANSACTION
        | BC_REPLY
        | BC_ACQUIRE_RESULT
        | BC_FREE_BUFFER
        | BC_INCREFS
        | BC_ACQUIRE
        | BC_RELEASE
        | BC_DECREFS
        | BC_INCREFS_DONE
        | BC_ACQUIRE_DONE
        | BC_ATTEMPT_ACQUIRE
        | BC_REGISTER_LOOPER
        | BC_ENTER_LOOPER
        | BC_EXIT_LOOPER
        | BC_REQUEST_DEATH_NOTIFICATION
        | BC_CLEAR_DEATH_NOTIFICATION
        | BC_DEAD_BINDER_DONE
        | BC_TRANSACTION_SG
        | BC_REPLY_SG
        | BC_REQUEST_FREEZE_NOTIFICATION
        | BC_CLEAR_FREEZE_NOTIFICATION
        | BC_FREEZE_NOTIFICATION_DONE => Some(ioc_size(cmd)),
        _ => None,
    }
}

/// Human-readable name of a BR code, for diagnostics and test failures.
pub fn return_name(cmd: u32) -> &'static str {
    match cmd {
        BR_ERROR => "BR_ERROR",
        BR_OK => "BR_OK",
        BR_TRANSACTION_SEC_CTX => "BR_TRANSACTION_SEC_CTX",
        BR_TRANSACTION => "BR_TRANSACTION",
        BR_REPLY => "BR_REPLY",
        BR_ACQUIRE_RESULT => "BR_ACQUIRE_RESULT",
        BR_DEAD_REPLY => "BR_DEAD_REPLY",
        BR_TRANSACTION_COMPLETE => "BR_TRANSACTION_COMPLETE",
        BR_INCREFS => "BR_INCREFS",
        BR_ACQUIRE => "BR_ACQUIRE",
        BR_RELEASE => "BR_RELEASE",
        BR_DECREFS => "BR_DECREFS",
        BR_ATTEMPT_ACQUIRE => "BR_ATTEMPT_ACQUIRE",
        BR_NOOP => "BR_NOOP",
        BR_SPAWN_LOOPER => "BR_SPAWN_LOOPER",
        BR_FINISHED => "BR_FINISHED",
        BR_DEAD_BINDER => "BR_DEAD_BINDER",
        BR_CLEAR_DEATH_NOTIFICATION_DONE => "BR_CLEAR_DEATH_NOTIFICATION_DONE",
        BR_FAILED_REPLY => "BR_FAILED_REPLY",
        BR_FROZEN_REPLY => "BR_FROZEN_REPLY",
        BR_ONEWAY_SPAM_SUSPECT => "BR_ONEWAY_SPAM_SUSPECT",
        BR_TRANSACTION_PENDING_FROZEN => "BR_TRANSACTION_PENDING_FROZEN",
        BR_FROZEN_BINDER => "BR_FROZEN_BINDER",
        BR_CLEAR_FREEZE_NOTIFICATION_DONE => "BR_CLEAR_FREEZE_NOTIFICATION_DONE",
        _ => "BR_<unknown>",
    }
}

pub(crate) fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
pub(crate) fn i32_at(bytes: &[u8], offset: usize) -> i32 {
    i32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
pub(crate) fn u64_at(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}
pub(crate) fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
pub(crate) fn put_i32(bytes: &mut [u8], offset: usize, value: i32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
pub(crate) fn put_u64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

/// `struct binder_write_read`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WriteRead {
    pub write_size: u64,
    pub write_consumed: u64,
    pub write_buffer: u64,
    pub read_size: u64,
    pub read_consumed: u64,
    pub read_buffer: u64,
}

impl WriteRead {
    pub fn decode(bytes: &[u8]) -> Self {
        Self {
            write_size: u64_at(bytes, 0),
            write_consumed: u64_at(bytes, 8),
            write_buffer: u64_at(bytes, 16),
            read_size: u64_at(bytes, 24),
            read_consumed: u64_at(bytes, 32),
            read_buffer: u64_at(bytes, 40),
        }
    }

    pub fn encode(&self) -> [u8; WRITE_READ_SIZE] {
        let mut bytes = [0; WRITE_READ_SIZE];
        put_u64(&mut bytes, 0, self.write_size);
        put_u64(&mut bytes, 8, self.write_consumed);
        put_u64(&mut bytes, 16, self.write_buffer);
        put_u64(&mut bytes, 24, self.read_size);
        put_u64(&mut bytes, 32, self.read_consumed);
        put_u64(&mut bytes, 40, self.read_buffer);
        bytes
    }
}

/// `struct binder_transaction_data`. `target` is the handle for a BC and the
/// node pointer for a BR; `buffer`/`offsets` are the `data.ptr` pair.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TransactionData {
    pub target: u64,
    pub cookie: u64,
    pub code: u32,
    pub flags: u32,
    pub sender_pid: i32,
    pub sender_euid: u32,
    pub data_size: u64,
    pub offsets_size: u64,
    pub buffer: u64,
    pub offsets: u64,
}

impl TransactionData {
    pub fn decode(bytes: &[u8]) -> Self {
        Self {
            target: u64_at(bytes, 0),
            cookie: u64_at(bytes, 8),
            code: u32_at(bytes, 16),
            flags: u32_at(bytes, 20),
            sender_pid: i32_at(bytes, 24),
            sender_euid: u32_at(bytes, 28),
            data_size: u64_at(bytes, 32),
            offsets_size: u64_at(bytes, 40),
            buffer: u64_at(bytes, 48),
            offsets: u64_at(bytes, 56),
        }
    }

    pub fn encode(&self) -> [u8; TRANSACTION_DATA_SIZE] {
        let mut bytes = [0; TRANSACTION_DATA_SIZE];
        put_u64(&mut bytes, 0, self.target);
        put_u64(&mut bytes, 8, self.cookie);
        put_u32(&mut bytes, 16, self.code);
        put_u32(&mut bytes, 20, self.flags);
        put_i32(&mut bytes, 24, self.sender_pid);
        put_u32(&mut bytes, 28, self.sender_euid);
        put_u64(&mut bytes, 32, self.data_size);
        put_u64(&mut bytes, 40, self.offsets_size);
        put_u64(&mut bytes, 48, self.buffer);
        put_u64(&mut bytes, 56, self.offsets);
        bytes
    }

    /// The BC target handle (the low 32 bits of the target union).
    pub fn handle(&self) -> u32 {
        self.target as u32
    }
}

/// `struct flat_binder_object` (also the argument of
/// `BINDER_SET_CONTEXT_MGR_EXT`). `binder` holds the handle for handle types.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FlatBinderObject {
    pub kind: u32,
    pub flags: u32,
    pub binder: u64,
    pub cookie: u64,
}

impl FlatBinderObject {
    pub fn decode(bytes: &[u8]) -> Self {
        Self {
            kind: u32_at(bytes, 0),
            flags: u32_at(bytes, 4),
            binder: u64_at(bytes, 8),
            cookie: u64_at(bytes, 16),
        }
    }

    pub fn encode(&self) -> [u8; FLAT_BINDER_OBJECT_SIZE] {
        let mut bytes = [0; FLAT_BINDER_OBJECT_SIZE];
        put_u32(&mut bytes, 0, self.kind);
        put_u32(&mut bytes, 4, self.flags);
        put_u64(&mut bytes, 8, self.binder);
        put_u64(&mut bytes, 16, self.cookie);
        bytes
    }

    pub fn handle(&self) -> u32 {
        self.binder as u32
    }
}

/// `struct binder_buffer_object` (BINDER_TYPE_PTR).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BufferObject {
    pub flags: u32,
    pub buffer: u64,
    pub length: u64,
    pub parent: u64,
    pub parent_offset: u64,
}

impl BufferObject {
    pub fn decode(bytes: &[u8]) -> Self {
        Self {
            flags: u32_at(bytes, 4),
            buffer: u64_at(bytes, 8),
            length: u64_at(bytes, 16),
            parent: u64_at(bytes, 24),
            parent_offset: u64_at(bytes, 32),
        }
    }

    pub fn encode(&self) -> [u8; BUFFER_OBJECT_SIZE] {
        let mut bytes = [0; BUFFER_OBJECT_SIZE];
        put_u32(&mut bytes, 0, BINDER_TYPE_PTR);
        put_u32(&mut bytes, 4, self.flags);
        put_u64(&mut bytes, 8, self.buffer);
        put_u64(&mut bytes, 16, self.length);
        put_u64(&mut bytes, 24, self.parent);
        put_u64(&mut bytes, 32, self.parent_offset);
        bytes
    }
}

/// `struct binder_fd_array_object` (BINDER_TYPE_FDA).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FdArrayObject {
    pub num_fds: u64,
    pub parent: u64,
    pub parent_offset: u64,
}

impl FdArrayObject {
    pub fn decode(bytes: &[u8]) -> Self {
        Self {
            num_fds: u64_at(bytes, 8),
            parent: u64_at(bytes, 16),
            parent_offset: u64_at(bytes, 24),
        }
    }

    pub fn encode(&self) -> [u8; FD_ARRAY_OBJECT_SIZE] {
        let mut bytes = [0; FD_ARRAY_OBJECT_SIZE];
        put_u32(&mut bytes, 0, BINDER_TYPE_FDA);
        put_u64(&mut bytes, 8, self.num_fds);
        put_u64(&mut bytes, 16, self.parent);
        put_u64(&mut bytes, 24, self.parent_offset);
        bytes
    }
}

/// The encoded size of an object of type `kind`, or `None` for an invalid type.
pub fn object_size(kind: u32) -> Option<usize> {
    match kind {
        BINDER_TYPE_BINDER
        | BINDER_TYPE_WEAK_BINDER
        | BINDER_TYPE_HANDLE
        | BINDER_TYPE_WEAK_HANDLE => Some(FLAT_BINDER_OBJECT_SIZE),
        BINDER_TYPE_FD => Some(FD_OBJECT_SIZE),
        BINDER_TYPE_PTR => Some(BUFFER_OBJECT_SIZE),
        BINDER_TYPE_FDA => Some(FD_ARRAY_OBJECT_SIZE),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Values as bionic's `<linux/android/binder.h>` computes them on arm64.
    #[test]
    fn ioctl_and_protocol_numbers_match_the_arm64_uapi() {
        assert_eq!(BINDER_WRITE_READ, 0xc030_6201);
        assert_eq!(BINDER_SET_MAX_THREADS, 0x4004_6205);
        assert_eq!(BINDER_SET_CONTEXT_MGR, 0x4004_6207);
        assert_eq!(BINDER_THREAD_EXIT, 0x4004_6208);
        assert_eq!(BINDER_VERSION, 0xc004_6209);
        assert_eq!(BINDER_GET_NODE_DEBUG_INFO, 0xc018_620b);
        assert_eq!(BINDER_GET_NODE_INFO_FOR_REF, 0xc018_620c);
        assert_eq!(BINDER_SET_CONTEXT_MGR_EXT, 0x4018_620d);
        assert_eq!(BINDER_FREEZE, 0x400c_620e);
        assert_eq!(BINDER_GET_FROZEN_INFO, 0xc00c_620f);
        assert_eq!(BINDER_ENABLE_ONEWAY_SPAM_DETECTION, 0x4004_6210);
        assert_eq!(BINDER_GET_EXTENDED_ERROR, 0xc00c_6211);
        assert_eq!(BC_TRANSACTION, 0x4040_6300);
        assert_eq!(BC_REPLY, 0x4040_6301);
        assert_eq!(BC_FREE_BUFFER, 0x4008_6303);
        assert_eq!(BC_INCREFS, 0x4004_6304);
        assert_eq!(BC_ENTER_LOOPER, 0x0000_630c);
        assert_eq!(BC_REQUEST_DEATH_NOTIFICATION, 0x400c_630e);
        assert_eq!(BC_TRANSACTION_SG, 0x4048_6311);
        assert_eq!(BR_TRANSACTION, 0x8040_7202);
        assert_eq!(BR_TRANSACTION_SEC_CTX, 0x8048_7202);
        assert_eq!(BR_REPLY, 0x8040_7203);
        assert_eq!(BR_NOOP, 0x0000_720c);
        assert_eq!(BR_TRANSACTION_COMPLETE, 0x0000_7206);
        assert_eq!(BR_INCREFS, 0x8010_7207);
        assert_eq!(BR_DEAD_BINDER, 0x8008_720f);
        assert_eq!(BINDER_TYPE_BINDER, 0x7362_2a85);
        assert_eq!(BINDER_TYPE_HANDLE, 0x7368_2a85);
        assert_eq!(BINDER_TYPE_FD, 0x6664_2a85);
    }

    #[test]
    fn structures_round_trip() {
        let tr = TransactionData {
            target: 7,
            cookie: 9,
            code: 3,
            flags: TF_ONE_WAY,
            sender_pid: -1,
            sender_euid: 10_000,
            data_size: 16,
            offsets_size: 8,
            buffer: 0x1000,
            offsets: 0x2000,
        };
        assert_eq!(TransactionData::decode(&tr.encode()), tr);
        let bwr = WriteRead {
            write_size: 1,
            write_consumed: 2,
            write_buffer: 3,
            read_size: 4,
            read_consumed: 5,
            read_buffer: 6,
        };
        assert_eq!(WriteRead::decode(&bwr.encode()), bwr);
        assert_eq!(command_payload_size(0x6300 | 99), None);
        assert_eq!(command_payload_size(BC_TRANSACTION_SG), Some(72));
    }
}
