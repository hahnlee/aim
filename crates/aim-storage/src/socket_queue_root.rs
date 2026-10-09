//! Darwin's Unix GC scans receive queues only through message-list fileglobs.
//! Keep the receiver on that list without retaining a separate open descriptor.
use std::{io,os::fd::{AsRawFd,BorrowedFd,OwnedFd},os::unix::net::UnixStream};
use crate::private_fd::{self,PrivateFd};

/// Before terminal close, the open-description owner calls prepare_last_close,
/// then closes its last receiver alias/pin and finally drops this root.
pub struct SocketQueueRoot { _sender:PrivateFd, _receiver:PrivateFd,identity:crate::socket_inode::Identity }
impl SocketQueueRoot {
    /// Call only once the owner has excluded further public aliases/pins.
    /// Flush queued rights while the receiver is still visible to Unix GC.
    pub fn prepare_last_close(&self,receiver:BorrowedFd<'_>)->io::Result<()>{
        if crate::socket_inode::identity(receiver.as_raw_fd())?!=self.identity{return Err(io::Error::from_raw_os_error(libc::EINVAL));}
        if unsafe{libc::shutdown(receiver.as_raw_fd(),libc::SHUT_RD)}<0{return Err(io::Error::last_os_error());}
        Ok(())
    }
    pub fn new(receiver:BorrowedFd<'_>)->io::Result<Self>{
        let identity=crate::socket_inode::identity(receiver.as_raw_fd())?;
        let(_,mut pair)=private_fd::receive_allocations(||{
            let(sender,keeper)=UnixStream::pair()?;
            Ok(((),vec![OwnedFd::from(sender),OwnedFd::from(keeper)]))
        })?;
        let keeper=pair.pop().unwrap();let sender=pair.pop().unwrap();
        let mut control=[0usize;4];let byte=[0u8];
        let mut iov=libc::iovec{iov_base:byte.as_ptr()as*mut _,iov_len:1};
        // SAFETY: aligned ancillary buffer containing exactly the borrowed fd.
        unsafe{
            let mut message:libc::msghdr=std::mem::zeroed();
            message.msg_iov=&mut iov;message.msg_iovlen=1;
            message.msg_control=control.as_mut_ptr().cast();message.msg_controllen=libc::CMSG_SPACE(4);
            let header=libc::CMSG_FIRSTHDR(&message);
            (*header).cmsg_level=libc::SOL_SOCKET;(*header).cmsg_type=libc::SCM_RIGHTS;(*header).cmsg_len=libc::CMSG_LEN(4);
            std::ptr::write_unaligned(libc::CMSG_DATA(header).cast::<i32>(),receiver.as_raw_fd());
            let count=libc::sendmsg(sender.as_raw_fd(),&message,libc::MSG_DONTWAIT);
            if count<0{return Err(io::Error::last_os_error());}
            if count!=1{return Err(io::Error::from_raw_os_error(libc::EIO));}
        }
        Ok(Self{_sender:sender,_receiver:keeper,identity})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::{Read,Write},os::fd::{AsFd,FromRawFd}};
    fn send(socket:&UnixStream,fd:BorrowedFd<'_>){
        let mut control=[0usize;4];let byte=[1u8];let mut iov=libc::iovec{iov_base:byte.as_ptr()as*mut _,iov_len:1};
        unsafe{
            let mut msg:libc::msghdr=std::mem::zeroed();msg.msg_iov=&mut iov;msg.msg_iovlen=1;msg.msg_control=control.as_mut_ptr().cast();msg.msg_controllen=libc::CMSG_SPACE(4);
            let c=libc::CMSG_FIRSTHDR(&msg);(*c).cmsg_level=libc::SOL_SOCKET;(*c).cmsg_type=libc::SCM_RIGHTS;(*c).cmsg_len=libc::CMSG_LEN(4);
            std::ptr::write_unaligned(libc::CMSG_DATA(c).cast::<i32>(),fd.as_raw_fd());assert_eq!(libc::sendmsg(socket.as_raw_fd(),&msg,0),1);
        }
    }
    fn receive(socket:&UnixStream)->UnixStream{
        let mut control=[0usize;4];let mut byte=[0u8];let mut iov=libc::iovec{iov_base:byte.as_mut_ptr().cast(),iov_len:1};
        unsafe{
            let mut msg:libc::msghdr=std::mem::zeroed();msg.msg_iov=&mut iov;msg.msg_iovlen=1;msg.msg_control=control.as_mut_ptr().cast();msg.msg_controllen=std::mem::size_of_val(&control)as u32;
            assert_eq!(libc::recvmsg(socket.as_raw_fd(),&mut msg,0),1);assert_eq!(msg.msg_flags&libc::MSG_CTRUNC,0);
            let c=libc::CMSG_FIRSTHDR(&msg);assert!(!c.is_null());assert_eq!((*c).cmsg_level,libc::SOL_SOCKET);assert_eq!((*c).cmsg_type,libc::SCM_RIGHTS);assert_eq!((*c).cmsg_len,libc::CMSG_LEN(4));
            UnixStream::from(OwnedFd::from_raw_fd(std::ptr::read_unaligned(libc::CMSG_DATA(c).cast::<i32>())))
        }
    }
    fn eof(peer:&UnixStream){let mut byte=0u8;assert_eq!(unsafe{libc::recv(peer.as_raw_fd(),(&mut byte as*mut u8).cast(),1,libc::MSG_DONTWAIT)},0,"keeper release must synchronously discard the abandoned queue");}
    fn live(peer:&UnixStream){let mut byte=0u8;assert_eq!(unsafe{libc::recv(peer.as_raw_fd(),(&mut byte as*mut u8).cast(),1,libc::MSG_DONTWAIT)},-1);assert_eq!(io::Error::last_os_error().kind(),io::ErrorKind::WouldBlock);}
    fn parallel(case:fn()){
        let workers:Vec<_>=(0..5).map(|_|std::thread::spawn(move||for _ in 0..1000{case()})).collect();
        for worker in workers{worker.join().unwrap();}
    }
    #[test]
    fn socket_queue_root_externalization_keeps_real_io_and_imported_last_close(){
        const MARKER:&str="SOCKET_ROOT_IMPORT_FIXTURE_EXECUTED";
        if crate::inode_lease::tests::isolated_fork_fixture("socket_queue_root::tests::socket_queue_root_externalization_keeps_real_io_and_imported_last_close",MARKER){return;}
        parallel(||{
        let(backing,mut peer)=UnixStream::pair().unwrap();let(sender,receiver)=UnixStream::pair().unwrap();let root=SocketQueueRoot::new(receiver.as_fd()).unwrap();
        send(&sender,backing.as_fd());drop(backing);let mut imported=receive(&receiver);root.prepare_last_close(receiver.as_fd()).unwrap();drop(receiver);drop(root);drop(sender);
        peer.write_all(b"real").unwrap();let mut bytes=[0;4];imported.read_exact(&mut bytes).unwrap();assert_eq!(&bytes,b"real");drop(imported);eof(&peer);
    });
        println!("{MARKER}");
    }
    #[test]
    fn socket_queue_root_abandoned_queue_releases_at_actual_receiver_then_keeper_close(){
        const MARKER:&str="SOCKET_ROOT_DISCARD_FIXTURE_EXECUTED";
        if crate::inode_lease::tests::isolated_fork_fixture("socket_queue_root::tests::socket_queue_root_abandoned_queue_releases_at_actual_receiver_then_keeper_close",MARKER){return;}
        parallel(||{
        let(backing,peer)=UnixStream::pair().unwrap();let(sender,receiver)=UnixStream::pair().unwrap();let root=SocketQueueRoot::new(receiver.as_fd()).unwrap();
        send(&sender,backing.as_fd());drop(backing);live(&peer);root.prepare_last_close(receiver.as_fd()).unwrap();drop(receiver);drop(root);drop(sender);eof(&peer);
    });
        println!("{MARKER}");
    }
    #[test]
    fn socket_queue_root_last_receiver_alias_controls_queued_right_lifetime(){
        const MARKER:&str="SOCKET_ROOT_ALIAS_FIXTURE_EXECUTED";
        if crate::inode_lease::tests::isolated_fork_fixture("socket_queue_root::tests::socket_queue_root_last_receiver_alias_controls_queued_right_lifetime",MARKER){return;}
        parallel(||{
        let(backing,peer)=UnixStream::pair().unwrap();let(sender,receiver)=UnixStream::pair().unwrap();let root=SocketQueueRoot::new(receiver.as_fd()).unwrap();let alias=receiver.try_clone().unwrap();
        send(&sender,backing.as_fd());drop(backing);drop(receiver);live(&peer);root.prepare_last_close(alias.as_fd()).unwrap();drop(alias);drop(root);drop(sender);eof(&peer);
    });
        println!("{MARKER}");
    }
}
