//! Linux errno values and the Darwin -> Linux mapping.

/// A Linux errno value (positive).
pub type Errno = i32;

pub const EPERM: Errno = 1;
pub const ENOENT: Errno = 2;
pub const ESRCH: Errno = 3;
pub const EINTR: Errno = 4;
pub const EIO: Errno = 5;
pub const E2BIG: Errno = 7;
pub const ENOEXEC: Errno = 8;
pub const EBADF: Errno = 9;
pub const EAGAIN: Errno = 11;
pub const ENOMEM: Errno = 12;
pub const EACCES: Errno = 13;
pub const EFAULT: Errno = 14;
pub const EBUSY: Errno = 16;
pub const EEXIST: Errno = 17;
pub const EXDEV: Errno = 18;
pub const EISDIR: Errno = 21;
pub const EROFS: Errno = 30;
pub const EOPNOTSUPP: Errno = 95;
pub const ENODEV: Errno = 19;
pub const ENOTDIR: Errno = 20;
pub const EINVAL: Errno = 22;
pub const ENOTTY: Errno = 25;
pub const ENOSPC: Errno = 28;
pub const ERANGE: Errno = 34;
pub const ENOSYS: Errno = 38;
pub const ELOOP: Errno = 40;

/// Translate a Darwin errno into its Linux value.
pub const fn from_darwin(e: i32) -> Errno {
    use libc as d;
    match e {
        // 1..=34 coincide, except Darwin 11 (EDEADLK) and 35 (EAGAIN).
        d::EDEADLK => 35,
        d::EAGAIN => 11,
        1..=10 | 12..=34 => e,
        d::EINPROGRESS => 115,
        d::EALREADY => 114,
        d::ENOTSOCK => 88,
        d::EDESTADDRREQ => 89,
        d::EMSGSIZE => 90,
        d::EPROTOTYPE => 91,
        d::ENOPROTOOPT => 92,
        d::EPROTONOSUPPORT => 93,
        d::ESOCKTNOSUPPORT => 94,
        d::ENOTSUP | d::EOPNOTSUPP => 95,
        d::EPFNOSUPPORT => 96,
        d::EAFNOSUPPORT => 97,
        d::EADDRINUSE => 98,
        d::EADDRNOTAVAIL => 99,
        d::ENETDOWN => 100,
        d::ENETUNREACH => 101,
        d::ENETRESET => 102,
        d::ECONNABORTED => 103,
        d::ECONNRESET => 104,
        d::ENOBUFS => 105,
        d::EISCONN => 106,
        d::ENOTCONN => 107,
        d::ESHUTDOWN => 108,
        d::ETOOMANYREFS => 109,
        d::ETIMEDOUT => 110,
        d::ECONNREFUSED => 111,
        d::ELOOP => 40,
        d::ENAMETOOLONG => 36,
        d::EHOSTDOWN => 112,
        d::EHOSTUNREACH => 113,
        d::ENOTEMPTY => 39,
        d::EUSERS => 87,
        d::EDQUOT => 122,
        d::ESTALE => 116,
        d::EREMOTE => 66,
        d::ENOLCK => 37,
        d::ENOSYS => 38,
        d::EOVERFLOW => 75,
        d::ECANCELED => 125,
        d::EIDRM => 43,
        d::ENOMSG => 42,
        d::EILSEQ => 84,
        d::ENOATTR => 61, // ENODATA
        d::EBADMSG => 74,
        d::EMULTIHOP => 72,
        d::ENODATA => 61,
        d::ENOLINK => 67,
        d::ENOSR => 63,
        d::ENOSTR => 60,
        d::EPROTO => 71,
        d::ETIME => 62,
        d::ENOTRECOVERABLE => 131,
        d::EOWNERDEAD => 130,
        _ => EIO,
    }
}

/// Darwin errno -> Linux errno for errnos below 128, for the lean syscall
/// path in `trampoline.S` (it maps anything larger to EIO).
pub static DARWIN_TO_LINUX: [u8; 128] = {
    let mut t = [0u8; 128];
    let mut i = 0;
    while i < 128 {
        t[i] = from_darwin(i as i32) as u8;
        i += 1;
    }
    t
};

/// The current thread's Darwin errno, as a Linux value.
pub fn last() -> Errno {
    from_darwin(
        std::io::Error::last_os_error()
            .raw_os_error()
            .unwrap_or(libc::EIO),
    )
}

/// Convert a Darwin libc return value into a Linux syscall result.
pub fn check(r: i64) -> i64 {
    if r < 0 { -(last() as i64) } else { r }
}
