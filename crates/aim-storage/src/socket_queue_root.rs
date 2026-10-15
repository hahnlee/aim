//! Darwin's Unix GC scans receive queues only through message-list fileglobs.
//! Keep the receiver on that list without retaining a separate open descriptor.
use std::{io,os::fd::{AsRawFd,BorrowedFd,OwnedFd,FromRawFd},os::unix::net::UnixStream};
use crate::private_fd::{self,PrivateFd};

/// Before terminal close, the open-description owner calls prepare_last_close,
/// then closes its last receiver alias/pin and finally drops this root.
pub struct SocketQueueRoot { _sender:PrivateFd, _receiver:PrivateFd,identity:crate::socket_inode::Identity }
impl SocketQueueRoot {
    /// Call only once the owner has excluded further public aliases/pins.
    /// Flush queued rights while the receiver is still visible to Unix GC.
    pub fn prepare_last_close(&self,receiver:BorrowedFd<'_>)->io::Result<()>{
        if crate::socket_inode::identity(receiver.as_raw_fd())?!=self.identity{return Err(io::Error::from_raw_os_error(libc::EINVAL));}
        if unsafe{libc::shutdown(receiver.as_raw_fd(),libc::SHUT_RD)}==0{return Ok(());}
        let failure=io::Error::last_os_error();
        if failure.raw_os_error()!=Some(libc::ENOTCONN){return Err(failure);}
        // XNU rejects shutdown before sorflush once the peer is disconnected.
        // Externalize and close queued rights while GC membership remains live.
        let deadline=std::time::Instant::now()+std::time::Duration::from_secs(1);
        loop{
            let mut consumed=false;
            let received=private_fd::receive_allocations(||{
                let mut data=[0u8;4096];let mut control=[0usize;1024];
                let mut iov=libc::iovec{iov_base:data.as_mut_ptr().cast(),iov_len:data.len()};
                unsafe{
                    let mut message:libc::msghdr=std::mem::zeroed();message.msg_iov=&mut iov;message.msg_iovlen=1;
                    message.msg_control=control.as_mut_ptr().cast();message.msg_controllen=std::mem::size_of_val(&control)as u32;
                    let count=libc::recvmsg(receiver.as_raw_fd(),&mut message,libc::MSG_DONTWAIT);
                    if count<0{return Err(io::Error::last_os_error());}
                    consumed=true;
                    let mut descriptors=vec![];let mut header=libc::CMSG_FIRSTHDR(&message);
                    while !header.is_null(){
                        if (*header).cmsg_level==libc::SOL_SOCKET&&(*header).cmsg_type==libc::SCM_RIGHTS{
                            let length=(*header).cmsg_len.saturating_sub(libc::CMSG_LEN(0));
                            for offset in 0..length/4{descriptors.push(OwnedFd::from_raw_fd(std::ptr::read_unaligned(libc::CMSG_DATA(header).add(offset as usize*4).cast::<i32>())));}
                        }
                        header=libc::CMSG_NXTHDR(&message,header);
                    }
                    for descriptor in &descriptors{
                        let flags=libc::fcntl(descriptor.as_raw_fd(),libc::F_GETFD);
                        if flags<0||libc::fcntl(descriptor.as_raw_fd(),libc::F_SETFD,flags|libc::FD_CLOEXEC)<0{return Err(io::Error::last_os_error());}
                    }
                    Ok(((count,message.msg_flags),descriptors))
                }
            });
            let((count,flags),descriptors)=match received{
                Ok(received)=>received,
                Err(failure)if !consumed&&matches!(failure.kind(),io::ErrorKind::Interrupted|io::ErrorKind::WouldBlock)=>{
                    loop{
                        let Some(remaining)=deadline.checked_duration_since(std::time::Instant::now())else{return Err(io::Error::new(io::ErrorKind::TimedOut,"disconnected receive cleanup timeout"));};
                        let mut event=libc::pollfd{fd:receiver.as_raw_fd(),events:libc::POLLIN,revents:0};
                        let ready=unsafe{libc::poll(&mut event,1,remaining.as_millis().max(1).min(i32::MAX as u128)as i32)};
                        if ready<0{let failure=io::Error::last_os_error();if failure.kind()==io::ErrorKind::Interrupted{continue;}return Err(failure);}
                        if ready==0{continue;}if event.revents&libc::POLLNVAL!=0{return Err(io::Error::from_raw_os_error(libc::EBADF));}break;
                    }
                    continue;
                },
                Err(failure)=>return Err(failure),
            };
            drop(descriptors);
            if flags&(libc::MSG_CTRUNC|libc::MSG_TRUNC)!=0{return Err(io::Error::from_raw_os_error(libc::EPROTO));}
            if count==0{return Ok(());}
        }
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
    }    #[test]
    fn disconnected_terminal_cleanup_closes_queued_rights_and_preserves_owner_errors(){
        const MARKER:&str="SOCKET_ROOT_DISCONNECTED_FIXTURE_EXECUTED";
        if crate::inode_lease::tests::isolated_fork_fixture("socket_queue_root::tests::disconnected_terminal_cleanup_closes_queued_rights_and_preserves_owner_errors",MARKER){return;}
        parallel(||{
            let(backing,peer)=UnixStream::pair().unwrap();let(sender,receiver)=UnixStream::pair().unwrap();let root=SocketQueueRoot::new(receiver.as_fd()).unwrap();
            assert_eq!(root.prepare_last_close(peer.as_fd()).err().unwrap().raw_os_error(),Some(libc::EINVAL));
            send(&sender,backing.as_fd());drop(backing);drop(sender);live(&peer);
            root.prepare_last_close(receiver.as_fd()).unwrap();eof(&peer);drop(receiver);drop(root);
            let(sender,receiver)=UnixStream::pair().unwrap();let root=SocketQueueRoot::new(receiver.as_fd()).unwrap();
            assert_eq!(unsafe{libc::shutdown(receiver.as_raw_fd(),libc::SHUT_RD)},0);root.prepare_last_close(receiver.as_fd()).unwrap();drop(sender);drop(receiver);drop(root);
        });
        println!("{MARKER}");
    }

}
