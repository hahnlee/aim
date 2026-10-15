//! Actual /dev/fuse syscall routing. One device read/write is one FUSE packet.
use crate::errno::{Errno,EINVAL,EIO};
use super::fuse;
pub fn is_device(fd:i32)->bool{fuse::marker(fd)==Some(fuse::DEVICE_MARKER)}
pub fn is_typed(fd:i32)->bool{matches!(fuse::marker(fd),Some(fuse::DEVICE_MARKER|fuse::FILE_MARKER))}
pub fn adopt(fd:i32)->bool{if !is_device(fd){return false;}if let Some(slow)=super::fdtab::SLOW.get(fd as usize){slow.store(1,std::sync::atomic::Ordering::Relaxed);}true}
pub fn rw(fd:i32,vectors:&[libc::iovec],write:bool)->Option<i64>{
    if !is_device(fd){return None;}let size=match vectors.iter().try_fold(0usize,|size,vector|size.checked_add(vector.iov_len)){Some(size)if size<=fuse::MAX_MESSAGE=>size,_=>return Some(-(EINVAL as i64))};
    let mut packet=vec![0u8;size];
    let result=if write{let mut offset=0;for vector in vectors{unsafe{std::ptr::copy_nonoverlapping(vector.iov_base.cast::<u8>(),packet[offset..].as_mut_ptr(),vector.iov_len);}offset+=vector.iov_len;}fuse::write_device(fd,&packet)}
    else{let nonblock=unsafe{libc::fcntl(fd,libc::F_GETFL)}&libc::O_NONBLOCK!=0;fuse::read_device(fd,&mut packet,nonblock)};
    match result{Err(error)=>Some(-(error as i64)),Ok(count)=>{if count>size{return Some(-(EIO as i64));}if !write{let mut offset=0;for vector in vectors{let amount=vector.iov_len.min(count-offset);unsafe{std::ptr::copy_nonoverlapping(packet[offset..].as_ptr(),vector.iov_base.cast::<u8>(),amount);}offset+=amount;if offset==count{break;}}}Some(count as i64)}}
}
pub fn readiness(fd:i32)->Option<Result<i16,Errno>>{is_device(fd).then(||fuse::poll_device(fd).map(|events|events as i16))}
pub fn stat(fd:i32)->Option<libc::stat>{if !is_device(fd){return None;}let mut stat:libc::stat=unsafe{std::mem::zeroed()};stat.st_mode=libc::S_IFCHR|0o666;stat.st_nlink=1;stat.st_rdev=(10<<24)|229;stat.st_blksize=4096;Some(stat)}
