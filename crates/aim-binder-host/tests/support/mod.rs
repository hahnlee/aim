//! What the daemon's tests share: this process as the guest, and
//! BINDER_WRITE_READ built and parsed as IPCThreadState does.

use aim_binder_driver::uapi::*;
use aim_binder_driver::{Device, Errno};
use aim_binder_host::client::{BinderFile, Client, UserMemory};

/// This process's memory stands in for the guest's.
pub struct Own;

impl UserMemory for Own {
    fn read(&mut self, address: u64, len: usize) -> Result<Vec<u8>, Errno> {
        // SAFETY: the test passes pointers to its own live buffers.
        Ok(unsafe { std::slice::from_raw_parts(address as *const u8, len) }.to_vec())
    }

    fn write(&mut self, address: u64, data: &[u8]) -> Result<(), Errno> {
        // SAFETY: as above.
        unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), address as *mut u8, data.len()) };
        Ok(())
    }
}

pub const MAP: usize = 1 << 20;

pub fn open(client: &Client, euid: u32, context: &str) -> BinderFile {
    let file = client
        .open(Device::Binder, false, true, euid, context)
        .unwrap();
    // SAFETY: reserving the range the receive buffer replaces.
    let base = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            MAP,
            libc::PROT_NONE,
            libc::MAP_PRIVATE | libc::MAP_ANON,
            -1,
            0,
        )
    };
    file.mmap(base as u64, MAP as u64).unwrap();
    file
}

/// One BINDER_WRITE_READ; returns the read bytes.
pub fn write_read(file: &BinderFile, tid: i32, write: &[u8], read: bool) -> Result<Vec<u8>, Errno> {
    let mut buf = vec![0u8; if read { 512 } else { 0 }];
    let bwr = WriteRead {
        write_size: write.len() as u64,
        write_buffer: write.as_ptr() as u64,
        read_size: buf.len() as u64,
        read_buffer: buf.as_mut_ptr() as u64,
        ..Default::default()
    };
    let mut arg = bwr.encode();
    file.ioctl(tid, BINDER_WRITE_READ, arg.as_mut_ptr() as u64, &mut Own)?;
    let done = WriteRead::decode(&arg);
    assert_eq!(done.write_consumed, write.len() as u64);
    buf.truncate(done.read_consumed as usize);
    Ok(buf)
}

pub fn cmd(out: &mut Vec<u8>, code: u32, payload: &[u8]) {
    out.extend_from_slice(&code.to_le_bytes());
    out.extend_from_slice(payload);
}

/// The BR codes of a read, with each transaction's data.
pub fn returns(read: &[u8]) -> Vec<(u32, Option<TransactionData>)> {
    let mut out = Vec::new();
    let mut at = 0;
    while at + 4 <= read.len() {
        let code = u32::from_le_bytes(read[at..at + 4].try_into().unwrap());
        at += 4;
        let tr = matches!(code, BR_TRANSACTION | BR_TRANSACTION_SEC_CTX | BR_REPLY)
            .then(|| TransactionData::decode(&read[at..]));
        out.push((code, tr));
        at += ioc_size(code);
    }
    out
}
