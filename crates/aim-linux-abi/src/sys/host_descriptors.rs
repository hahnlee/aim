//! Guest handoff of native host-call pipe endpoints (#1211, #1212).
use super::fdtab;
use crate::errno::{self,Errno};
use std::os::fd::{AsRawFd,BorrowedFd};

fn publish(source:BorrowedFd<'_>)->Result<i32,Errno>{
    let _guard=fdtab::lifecycle();
    let fd=unsafe{libc::fcntl(source.as_raw_fd(),libc::F_DUPFD_CLOEXEC,0)};
    if fd<0{return Err(errno::last());}
    if let Err(error)=fdtab::publish_typed_guest(fd){unsafe{libc::close(fd);}return Err(error);}
    Ok(fd)
}

pub fn init(){
    static INSTALLED:std::sync::Once=std::sync::Once::new();
    INSTALLED.call_once(||{
        assert!(aim_host_memory::set_hooks(aim_host_memory::Hooks{publish}).is_ok(),"memory descriptor hook already installed");
        assert!(aim_host_bluetooth::set_hooks(aim_host_bluetooth::Hooks{publish}).is_ok(),"Bluetooth descriptor hook already installed");
    });
}

#[cfg(test)]mod tests{
    use super::*;
    #[test]fn memory_watcher_returns_a_public_pipe_and_keeps_native_writers_private(){
        if fdtab::isolated_kernel_test("sys::host_descriptors::tests::memory_watcher_returns_a_public_pipe_and_keeps_native_writers_private"){return;}
        let(_view,_root)=crate::vfs::test_view();fdtab::install_storage_registrar().unwrap();init();
        let hidden=|| (0..10240).filter(|fd|fdtab::is_hidden(*fd)).collect::<Vec<_>>();let before=hidden();
        let fd=crate::hostcall::call(aim_hostcall::module::MEMORY as u64,aim_hostcall::memory::FN_WATCH as u64,0,0);
        assert!(fd>=0,"native watcher {fd}");assert!(fdtab::visible(fd as i32));
        assert_eq!(super::super::fs::fcntl([fd as u64,1,0,0,0,0]),1);
        let private:Vec<_>=hidden().into_iter().filter(|n|!before.contains(n)).collect();assert!(!private.is_empty());
        for raw in &private{assert!(!fdtab::visible(*raw));assert!(fdtab::pin_guest(*raw).is_err());}
        aim_host_memory::notify();
        let mut poll=libc::pollfd{fd:fd as i32,events:libc::POLLIN,revents:0};let timeout=[1i64,0];
        assert_eq!(super::super::poll::ppoll([&mut poll as*mut _ as u64,1,timeout.as_ptr()as u64,0,0,0]),1);
        let mut stat=[0u8;128];assert_eq!(super::super::fs::fstat([fd as u64,stat.as_mut_ptr()as u64,0,0,0,0]),0);
        let mut byte=0u8;assert_eq!(super::super::fs::read([fd as u64,&mut byte as*mut u8 as u64,1,0,0,0]),1);assert_eq!(byte,1);
        assert_eq!(super::super::fs::close([fd as u64,0,0,0,0,0]),0);assert!(!fdtab::visible(fd as i32));
        aim_host_memory::notify();for raw in private{assert_eq!(unsafe{libc::fcntl(raw,libc::F_GETFD)},-1);}
    }
    struct Radio;
    impl aim_host_bluetooth::Backend for Radio{
        fn scan(&mut self,_on:bool,_duplicates:bool){}
        fn connect(&mut self,_peer:aim_host_bluetooth::BackendPeerId){}
        fn disconnect(&mut self,_peer:aim_host_bluetooth::BackendPeerId){}
        fn discover(&mut self,_peer:aim_host_bluetooth::BackendPeerId){}
        fn gatt(&mut self,_peer:aim_host_bluetooth::BackendPeerId,_op:aim_host_bluetooth::BackendOperation){}
        fn advertise(&mut self,_request:Option<aim_host_bluetooth::BackendAdvertisement>){}
    }
    #[test]fn bluetooth_injected_controller_wake_fd_uses_actual_guest_descriptor_owner(){
        if fdtab::isolated_kernel_test("sys::host_descriptors::tests::bluetooth_injected_controller_wake_fd_uses_actual_guest_descriptor_owner"){return;}
        let(_view,_root)=crate::vfs::test_view();fdtab::install_storage_registrar().unwrap();init();
        let hidden=|| (0..10240).filter(|fd|fdtab::is_hidden(*fd)).collect::<Vec<_>>();let before=hidden();
        let fd=aim_host_bluetooth::open_controller(Box::new(Radio),aim_host_bluetooth::Addr([1,2,3,4,5,0x02])).unwrap();
        assert!(fdtab::visible(fd));assert_eq!(super::super::fs::fcntl([fd as u64,1,0,0,0,0]),1);
        let private:Vec<_>=hidden().into_iter().filter(|n|!before.contains(n)).collect();assert!(private.len()>=2);
        for raw in &private{assert!(!fdtab::visible(*raw));assert!(fdtab::pin_guest(*raw).is_err());}
        let command=[3u8,12,0];let mut packet=aim_hostcall::bluetooth::Packet{kind:1,len:3,capacity:0,data:command.as_ptr()as u64};
        assert_eq!(crate::hostcall::call(aim_hostcall::module::BLUETOOTH as u64,aim_hostcall::bluetooth::FN_SEND as u64,&mut packet as*mut _ as u64,std::mem::size_of_val(&packet)as u64),0);
        let mut poll=libc::pollfd{fd,events:libc::POLLIN,revents:0};let timeout=[1i64,0];assert_eq!(super::super::poll::ppoll([&mut poll as*mut _ as u64,1,timeout.as_ptr()as u64,0,0,0]),1);
        let mut stat=[0u8;128];assert_eq!(super::super::fs::fstat([fd as u64,stat.as_mut_ptr()as u64,0,0,0,0]),0);
        let mut byte=0u8;assert_eq!(super::super::fs::read([fd as u64,&mut byte as*mut u8 as u64,1,0,0,0]),1);
        let mut event=[0u8;32];packet=aim_hostcall::bluetooth::Packet{kind:0,len:0,capacity:32,data:event.as_mut_ptr()as u64};
        assert_eq!(crate::hostcall::call(aim_hostcall::module::BLUETOOTH as u64,aim_hostcall::bluetooth::FN_RECV as u64,&mut packet as*mut _ as u64,std::mem::size_of_val(&packet)as u64),0);assert_eq!(&event[..packet.len as usize],&[14,4,1,3,12,0]);
        assert_eq!(super::super::fs::close([fd as u64,0,0,0,0,0]),0);
        assert_eq!(crate::hostcall::call(aim_hostcall::module::BLUETOOTH as u64,aim_hostcall::bluetooth::FN_CLOSE as u64,0,0),0);
        for raw in private{assert_eq!(unsafe{libc::fcntl(raw,libc::F_GETFD)},-1);}
    }

}
