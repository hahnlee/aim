//! splice, sendfile and copy_file_range as copies through a buffer. Darwin
//! has no in-kernel pipe splicing; the data and offsets come out as Linux
//! defines them.

use super::fs;
use crate::errno::EINVAL;

const CHUNK: usize = 64 << 10;

/// Copy up to `len` bytes from `fin` to `fout`. An offset pointer, when
/// given, is used and advanced instead of the file position.
fn copy(fin: i32, off_in: u64, fout: i32, off_out: u64, len: usize) -> i64 {
    let mut buf = vec![0u8; CHUNK.min(len.max(1))];
    let mut done = 0usize;
    while done < len {
        let want = buf.len().min(len - done);
        let p = buf.as_mut_ptr() as u64;
        let n = if off_in != 0 {
            // SAFETY: guest loff_t.
            let o = unsafe { (off_in as *const i64).read_unaligned() };
            let n = fs::pread64([fin as u64, p, want as u64, o as u64, 0, 0]);
            if n > 0 {
                // SAFETY: guest loff_t.
                unsafe { (off_in as *mut i64).write_unaligned(o + n) };
            }
            n
        } else {
            fs::read([fin as u64, p, want as u64, 0, 0, 0])
        };
        if n <= 0 {
            return if done > 0 { done as i64 } else { n };
        }
        let mut w = 0usize;
        while w < n as usize {
            let q = p + w as u64;
            let left = (n as usize - w) as u64;
            let m = if off_out != 0 {
                // SAFETY: guest loff_t.
                let o = unsafe { (off_out as *const i64).read_unaligned() };
                let m = fs::pwrite64([fout as u64, q, left, o as u64, 0, 0]);
                if m > 0 {
                    // SAFETY: guest loff_t.
                    unsafe { (off_out as *mut i64).write_unaligned(o + m) };
                }
                m
            } else {
                fs::write([fout as u64, q, left, 0, 0, 0])
            };
            if m <= 0 {
                let total = done + w;
                return if total > 0 { total as i64 } else { m };
            }
            w += m as usize;
        }
        done += n as usize;
        if (n as usize) < want {
            break;
        }
    }
    done as i64
}

pub fn splice(a: [u64; 6]) -> i64 {
    copy(a[0] as i32, a[1], a[2] as i32, a[3], a[4] as usize)
}

pub fn sendfile(a: [u64; 6]) -> i64 {
    copy(a[1] as i32, a[2], a[0] as i32, 0, a[3] as usize)
}

pub fn copy_file_range(a: [u64; 6]) -> i64 {
    if a[5] != 0 {
        return -(EINVAL as i64);
    }
    copy(a[0] as i32, a[1], a[2] as i32, a[3], a[4] as usize)
}
