use super::*;
use std::sync::mpsc;
struct Connection{key:SessionKey,fd:RawFd,worker:Option<std::thread::JoinHandle<Result<(),Errno>>>}
impl Connection{
 fn new()->Self{
  let directory=std::env::temp_dir().join(format!("af-{:x}",id().unwrap()));fs::create_dir(&directory).unwrap();let key=SessionKey(directory.join("c.sock"));let path=key.0.clone();let worker=std::thread::spawn(move||serve_broker(&path));
  while !key.0.exists(){std::thread::sleep(Duration::from_millis(1));}
  let fd=open_on_key(&key,0,OPEN_DEVICE,&[]).unwrap();Self{key,fd,worker:Some(worker)}
 }
}
impl Drop for Connection{fn drop(&mut self){if self.fd>=0{unsafe{libc::close(self.fd);}self.fd=-1;}if let Some(worker)=self.worker.take(){worker.join().unwrap().unwrap();}}}
fn next(fd:RawFd)->Vec<u8>{let mut bytes=vec![0;8192];let size=read_device(fd,&mut bytes,false).unwrap();bytes.truncate(size);bytes}
fn respond(fd:RawFd,request:&[u8],error:i32,body:&[u8]){let mut out=Vec::new();out.extend(((16+body.len()) as u32).to_le_bytes());out.extend(error.to_le_bytes());out.extend(&request[8..16]);out.extend(body);assert_eq!(write_device(fd,&out).unwrap(),out.len());}
fn initialize(connection:&Connection){Client::from_key(&connection.key).unwrap().mount().unwrap();let packet=next(connection.fd);assert_eq!(u32::from_le_bytes(packet[4..8].try_into().unwrap()),INIT);let mut out=vec![0;64];out[..4].copy_from_slice(&7u32.to_le_bytes());out[4..8].copy_from_slice(&39u32.to_le_bytes());out[20..24].copy_from_slice(&65536u32.to_le_bytes());respond(connection.fd,&packet,0,&out);}

#[test]
fn fuse_device_protocol_dup_clone_and_negative_reply(){
 let connection=Connection::new();initialize(&connection);
 assert_eq!(read_device(connection.fd,&mut [0;8192],true),Err(crate::errno::EAGAIN));
 assert_eq!(poll_device(connection.fd).unwrap(),4);
 let client=Client::from_key(&connection.key).unwrap();let task=std::thread::spawn(move||client.request(1,1,b"photo\0",10234,10235,54));
 let packet=next(connection.fd);assert_eq!(u32::from_le_bytes(packet[24..28].try_into().unwrap()),10234);
 let dup=unsafe{libc::dup(connection.fd)};assert!(dup>=0);assert_eq!(marker(dup),Some(DEVICE_MARKER));assert_eq!(session_for_fd(dup).unwrap(),connection.key);
 respond(dup,&packet,-2,&[]);assert_eq!(task.join().unwrap().err(),Some(2));unsafe{libc::close(dup);}
 let destination=Connection::new();ioctl_clone(destination.fd,connection.fd).unwrap();assert_eq!(session_for_fd(destination.fd).unwrap(),connection.key);
 let client=Client::from_key(&connection.key).unwrap();let task=std::thread::spawn(move||client.request(17,1,&[],0,0,1));let packet=next(destination.fd);respond(destination.fd,&packet,0,&[0;80]);assert_eq!(task.join().unwrap().unwrap().body.len(),80);
 let mut invalid=vec![0;16];invalid[..4].copy_from_slice(&16u32.to_le_bytes());invalid[8..16].copy_from_slice(&99999u64.to_le_bytes());assert_eq!(write_device(connection.fd,&invalid),Err(crate::errno::ENOENT));
}

#[test]
fn fuse_description_shared_offset_flags_directory_and_last_close(){
 let connection=Connection::new();initialize(&connection);
 let policy=MountPolicy{uid:0,gid:0,allow_other:true,default_permissions:false,read_only:false};
 let fd=open_description(&connection.key,5,71,2,false,"DCIM/photo","/storage/emulated/0/DCIM/photo",1,&policy,0,0,1).unwrap();let dup=unsafe{libc::dup(fd)};
 let metadata=description(dup).unwrap();assert_eq!(metadata.relative,"DCIM/photo");assert_eq!(metadata.open_flags,1);assert!(!metadata.policy.default_permissions);
 let task=std::thread::spawn(move||description_io_at(fd,false,None,&[],3));let packet=next(connection.fd);respond(connection.fd,&packet,0,b"abc");let reply=task.join().unwrap().unwrap();assert_eq!(reply.offset,0);assert_eq!(reply.body,b"abc");assert_eq!(offset(dup,None).unwrap(),3);
 assert_eq!(description_flags(dup,Some(0x400|0x800)).unwrap(),2|0x400|0x800);assert_eq!(description_flags(fd,None).unwrap(),2|0x400|0x800);
 let task=std::thread::spawn(move||description_io_at(fd,true,None,b"xy",2));let attr=next(connection.fd);assert_eq!(u32::from_le_bytes(attr[4..8].try_into().unwrap()),3);let mut out=vec![0;104];out[24..32].copy_from_slice(&10u64.to_le_bytes());respond(connection.fd,&attr,0,&out);let write=next(connection.fd);assert_eq!(u64::from_le_bytes(write[48..56].try_into().unwrap()),10);let mut result=[0;8];result[..4].copy_from_slice(&2u32.to_le_bytes());respond(connection.fd,&write,0,&result);assert_eq!(task.join().unwrap().unwrap().offset,10);assert_eq!(offset(dup,None).unwrap(),12);
 unsafe{libc::close(fd);}assert_eq!(offset(dup,None).unwrap(),12);unsafe{libc::close(dup);}let release=next(connection.fd);assert_eq!(u32::from_le_bytes(release[4..8].try_into().unwrap()),18);respond(connection.fd,&release,0,&[]);let forget=next(connection.fd);assert_eq!(u32::from_le_bytes(forget[4..8].try_into().unwrap()),FORGET);
 let fd=open_description(&connection.key,9,80,0,true,"DCIM","/storage/emulated/0/DCIM",0,&policy,0,0,1).unwrap();
 let task=std::thread::spawn(move||description_io_at(fd,false,None,&[],128));let request=next(connection.fd);let mut dirent=Vec::new();dirent.extend(44u64.to_le_bytes());dirent.extend(73u64.to_le_bytes());dirent.extend(3u32.to_le_bytes());dirent.extend(8u32.to_le_bytes());dirent.extend(b"one");dirent.resize(32,0);respond(connection.fd,&request,0,&dirent);task.join().unwrap().unwrap();assert_eq!(offset(fd,None).unwrap(),73);unsafe{libc::close(fd);}let release=next(connection.fd);respond(connection.fd,&release,0,&[]);next(connection.fd);
}

#[test]
fn fuse_descriptor_survives_real_process_inheritance_and_death_aborts_waiters(){
 let mut connection=Connection::new();initialize(&connection);let policy=MountPolicy{uid:0,gid:0,allow_other:true,default_permissions:false,read_only:false};let fd=open_description(&connection.key,2,22,2,false,"a","/storage/emulated/0/a",0,&policy,0,0,1).unwrap();
 let script=r#"import socket,sys,struct
s=socket.socket(fileno=int(sys.argv[1])); path=s.getpeername(); ident=int(s.getsockname().rsplit('.d',1)[1],16)
r=socket.socket(socket.AF_UNIX,socket.SOCK_STREAM);r.connect(path);r.sendall(b'AIMFUSE1'+struct.pack('<IQI',12,ident,8)+struct.pack('<Q',41));header=r.recv(8);error,n=struct.unpack('<iI',header);assert error==0;data=r.recv(n);assert struct.unpack('<Q',data)[0]==41
"#;
 use std::os::fd::FromRawFd;let inherited=unsafe{std::fs::File::from_raw_fd(libc::dup(fd))};
 let status=std::process::Command::new("/usr/bin/python3").arg("-c").arg(script).arg("0").stdin(std::process::Stdio::from(inherited)).status().unwrap();assert!(status.success());assert_eq!(offset(fd,None).unwrap(),41);
 unsafe{libc::close(fd);}let release=next(connection.fd);respond(connection.fd,&release,0,&[]);next(connection.fd);
 let client=Client::from_key(&connection.key).unwrap();let(sent,received)=mpsc::channel();let task=std::thread::spawn(move||{sent.send(()).unwrap();client.request(17,1,&[],0,0,1)});received.recv().unwrap();let _request=next(connection.fd);unsafe{libc::close(connection.fd);}connection.fd=-1;assert_eq!(task.join().unwrap().err(),Some(107));
}

#[test]
fn fuse_negotiated_chunks_notifications_and_borrowed_inode_lifetime(){
 let connection=Connection::new();Client::from_key(&connection.key).unwrap().mount().unwrap();let init=next(connection.fd);let mut init_out=vec![0;64];init_out[..4].copy_from_slice(&7u32.to_le_bytes());init_out[4..8].copy_from_slice(&39u32.to_le_bytes());init_out[20..24].copy_from_slice(&4u32.to_le_bytes());respond(connection.fd,&init,0,&init_out);
 let policy=MountPolicy{uid:0,gid:0,allow_other:true,default_permissions:false,read_only:false};
 let fd=open_description(&connection.key,6,72,2,false,"DCIM/a","/storage/emulated/0/DCIM/a",0,&policy,0,0,1).unwrap();
 let borrowed=borrow_inode_descriptor(&connection.key,6).unwrap();assert_eq!(description(borrowed.as_raw_fd()).unwrap().fh,72);assert_eq!(offset(borrowed.as_raw_fd(),Some(9)).unwrap(),9);assert_eq!(offset(fd,None).unwrap(),9);
 let client=Client::from_key(&connection.key).unwrap();let mut notify=Vec::new();notify.extend(40u32.to_le_bytes());notify.extend(2i32.to_le_bytes());notify.extend(0u64.to_le_bytes());notify.extend(6u64.to_le_bytes());notify.extend(9i64.to_le_bytes());notify.extend(10i64.to_le_bytes());assert_eq!(write_device(connection.fd,&notify).unwrap(),40);
 let notifications=client.take_notifications().unwrap();assert_eq!(notifications.len(),1);assert_eq!(notifications[0].code,2);assert_eq!(notifications[0].body,&notify[16..]);assert!(client.take_notifications().unwrap().is_empty());
 let task=std::thread::spawn(move||description_io_at(fd,true,None,b"abcdefghij",10));
 for (offset,length,data) in [(9u64,4u32,&b"abcd"[..]),(13,4,&b"efgh"[..]),(17,2,&b"ij"[..])]{let packet=next(connection.fd);assert_eq!(u64::from_le_bytes(packet[48..56].try_into().unwrap()),offset);assert_eq!(u32::from_le_bytes(packet[56..60].try_into().unwrap()),length);assert_eq!(&packet[80..],data);let mut body=[0;8];body[..4].copy_from_slice(&length.to_le_bytes());respond(connection.fd,&packet,0,&body);}
 let written=task.join().unwrap().unwrap();assert_eq!(written.offset,9);assert_eq!(written.body[..4],10u32.to_le_bytes());assert_eq!(offset(borrowed.as_raw_fd(),None).unwrap(),19);
 let task=std::thread::spawn(move||description_io_at(fd,false,Some(0),&[],10));for (offset,data) in [(0u64,&b"abcd"[..]),(4,&b"efgh"[..]),(8,&b"ij"[..])]{let packet=next(connection.fd);assert_eq!(u64::from_le_bytes(packet[48..56].try_into().unwrap()),offset);respond(connection.fd,&packet,0,data);}assert_eq!(task.join().unwrap().unwrap().body,b"abcdefghij");assert_eq!(offset(borrowed.as_raw_fd(),None).unwrap(),19);
 unsafe{libc::close(fd);}assert_eq!(description(borrowed.as_raw_fd()).unwrap().node,6);drop(borrowed);let release=next(connection.fd);assert_eq!(u32::from_le_bytes(release[4..8].try_into().unwrap()),18);respond(connection.fd,&release,0,&[]);let forget=next(connection.fd);assert_eq!(u32::from_le_bytes(forget[4..8].try_into().unwrap()),FORGET);assert_eq!(borrow_inode_descriptor(&connection.key,6).err(),Some(crate::errno::ENOENT));
}

#[test]
fn fuse_device_guest_syscall_vectors_clone_poll_and_character_metadata(){
 let connection=Connection::new();initialize(&connection);super::super::fuse_device::adopt(connection.fd);
 let fd=connection.fd;let client=Client::from_key(&connection.key).unwrap();let task=std::thread::spawn(move||client.request(1,1,b"image\0",10123,10124,42));
 let mut one=[0u8;17];let mut two=[0u8;8192];let mut vectors=[libc::iovec{iov_base:one.as_mut_ptr().cast(),iov_len:one.len()},libc::iovec{iov_base:two.as_mut_ptr().cast(),iov_len:two.len()}];
 let size=super::super::fs::readv([fd as u64,vectors.as_mut_ptr() as u64,2,0,0,0]);assert!(size>40);
 let mut packet=one.to_vec();packet.extend(&two[..size as usize-one.len()]);assert_eq!(u32::from_le_bytes(packet[4..8].try_into().unwrap()),1);
 let mut response=Vec::new();response.extend(16u32.to_le_bytes());response.extend((-2i32).to_le_bytes());response.extend(&packet[8..16]);
 let mut reply_vectors=[libc::iovec{iov_base:response.as_mut_ptr().cast(),iov_len:9},libc::iovec{iov_base:unsafe{response.as_mut_ptr().add(9)}.cast(),iov_len:7}];
 assert_eq!(super::super::fs::writev([fd as u64,reply_vectors.as_mut_ptr() as u64,2,0,0,0]),16);assert_eq!(task.join().unwrap().err(),Some(2));
 assert_eq!(super::super::fs::lseek([fd as u64,0,0,0,0,0]),-29);
 let mut stat=[0u8;128];assert_eq!(super::super::fs::fstat([fd as u64,stat.as_mut_ptr() as u64,0,0,0,0]),0);
 assert_eq!(u32::from_ne_bytes(stat[16..20].try_into().unwrap())&0o170000,0o020000);
 let rdev=u64::from_ne_bytes(stat[32..40].try_into().unwrap());assert_eq!(rdev,((10u64)<<8)|229);
 let mut poll=libc::pollfd{fd,events:4,revents:0};let timeout=libc::timespec{tv_sec:0,tv_nsec:0};assert_eq!(super::super::poll::ppoll([&mut poll as *mut _ as u64,1,&timeout as *const _ as u64,0,0,0]),1);assert_eq!(poll.revents&4,4);
 let destination=Connection::new();let source=fd;assert_eq!(super::super::fs::ioctl([destination.fd as u64,FUSE_DEV_IOC_CLONE,&source as *const i32 as u64,0,0,0]),0);assert_eq!(session_for_fd(destination.fd).unwrap(),connection.key);
 let mut value=0i32;let mut length=4u32;assert_eq!(super::super::net::getsockopt([fd as u64,1,3,&mut value as *mut _ as u64,&mut length as *mut _ as u64,0]),-88);
}

#[test]
fn long_guest_runtime_uses_short_transport_with_actual_linux_run_broker_cli(){
 let helper=aim_paths::root().join("target/release/linux-run");assert!(helper.is_file(),"release linux-run required for FUSE CLI integration");
 let long_runtime=std::env::temp_dir().join(format!("aim-fuse-runtime-{}",id().unwrap())).join("long-guest-runtime-component".repeat(8));fs::create_dir_all(&long_runtime).unwrap();
 use std::os::unix::ffi::OsStrExt;assert!(long_runtime.as_os_str().as_bytes().len()>104);
 let fd=open_device_with_helper(&long_runtime,0,&helper).unwrap();let key=session_for_fd(fd).unwrap();let directory=key.0.parent().unwrap().to_owned();
 assert!(key.0.with_extension("dffffffffffffffff").as_os_str().as_bytes().len()<104);
 assert_eq!(poll_device(fd).unwrap(),4);assert_eq!(fs::read_dir(long_runtime.join("fuse-sessions")).unwrap().count(),1);
 unsafe{libc::close(fd);}let deadline=std::time::Instant::now()+Duration::from_secs(5);
 while directory.exists()||fs::read_dir(long_runtime.join("fuse-sessions")).unwrap().count()!=0{assert!(std::time::Instant::now()<deadline,"owned FUSE transport did not retire");std::thread::sleep(Duration::from_millis(5));}
 assert!(!directory.exists());fs::remove_dir_all(long_runtime.ancestors().find(|path|path.file_name().is_some_and(|name|name.to_string_lossy().starts_with("aim-fuse-runtime-"))).unwrap()).unwrap();
}

#[test]
fn fusectl_lists_only_mounted_connections_and_abort_unblocks_guest_request(){
 let(_view,_root)=crate::vfs::test_view();let connection=Connection::new();
 let root="/sys/fs/fuse/connections";
 let before=match super::super::fuse_sysfs::node(root).unwrap().unwrap(){super::super::procfs::Node::Dir(entries)=>entries.len(),_=>panic!("fusectl root must be directory")};
 let number=u64::from_str_radix(connection.key.transport().parent().unwrap().file_name().unwrap().to_str().unwrap().strip_prefix("af-").unwrap(),16).unwrap();
 assert!(!crate::vfs::runtime_dir().unwrap().join("fuse-connections").join(number.to_string()).exists());
 initialize(&connection);assert_eq!(super::super::fuse_sysfs::mounted(&connection.key).unwrap(),number);
 let entries=match super::super::fuse_sysfs::node(root).unwrap().unwrap(){super::super::procfs::Node::Dir(entries)=>entries,_=>panic!("fusectl root must be directory")};
 assert_eq!(entries.len(),before+1);assert!(entries.iter().any(|entry|entry.name==number.to_string().as_bytes()));
 let client=Client::from_key(&connection.key).unwrap();let task=std::thread::spawn(move||client.request(1,1,b"pending\0",0,0,1));let packet=next(connection.fd);assert_eq!(u32::from_le_bytes(packet[4..8].try_into().unwrap()),1);
 let path=format!("{root}/{number}/abort");let fd=super::super::fuse_sysfs::open(&path,1).unwrap();assert!(fd>=0);
 let byte=b"1";assert_eq!(super::super::fs::write([fd as u64,byte.as_ptr() as u64,1,0,0,0]),1);super::super::fs::close([fd as u64,0,0,0,0,0]);
 assert_eq!(task.join().unwrap().err(),Some(107));
 fs::remove_file(crate::vfs::runtime_dir().unwrap().join("fuse-connections").join(number.to_string())).unwrap();
}

#[test]
fn bind_mount_directory_checks_use_fuse_daemon_not_missing_anchor_children(){
 let(_view,root)=crate::vfs::test_view();let connection=Connection::new();initialize(&connection);
 let anchor=root.join("bind-fuse-anchor");fs::create_dir_all(&anchor).unwrap();let lower=root.join("bind-fuse-lower");fs::create_dir_all(&lower).unwrap();
 let options=super::super::fuse_mount::parse(b"fd=17,rootmode=40000,user_id=0,group_id=0,allow_other,").unwrap();
 crate::vfs::add_fuse_mount("/mnt/aim-bind-fuse",connection.key.transport().to_owned(),anchor.clone(),"fuse",&options,false).unwrap();
 crate::vfs::add_mount("/mnt/aim-bind-lower",lower,crate::vfs::Area::Writable,"lower","bind");
 let source=std::ffi::CString::new("/mnt/aim-bind-lower").unwrap();let target=std::ffi::CString::new("/mnt/aim-bind-fuse/Android").unwrap();
 assert!(!anchor.join("Android").exists());
 let(done,completed)=mpsc::channel();
 let task=std::thread::spawn(move||{let result=super::super::mount::mount([source.as_ptr() as u64,target.as_ptr() as u64,0,4096,0,0]);done.send(result).unwrap();});
 let deadline=std::time::Instant::now()+Duration::from_secs(5);
 loop{if let Ok(result)=completed.try_recv(){assert_eq!(result,0);break;}assert!(std::time::Instant::now()<deadline);if poll_device(connection.fd).unwrap()&1==0{std::thread::sleep(Duration::from_millis(1));continue;}
  let packet=next(connection.fd);let opcode=u32::from_le_bytes(packet[4..8].try_into().unwrap());let node=u64::from_le_bytes(packet[16..24].try_into().unwrap());
  match opcode{
   1=>{assert_eq!(&packet[40..],b"Android\0");let mut entry=vec![0;128];entry[..8].copy_from_slice(&2u64.to_le_bytes());entry[100..104].copy_from_slice(&0o040755u32.to_le_bytes());respond(connection.fd,&packet,0,&entry);},
   3=>{assert!(node==1||node==2);let mut attr=vec![0;104];attr[76..80].copy_from_slice(&0o040755u32.to_le_bytes());respond(connection.fd,&packet,0,&attr);},
   FORGET=>{},_=>panic!("unexpected bind request {opcode}")
  }
 }
 task.join().unwrap();assert!(!anchor.join("Android").exists());
 assert!(crate::vfs::remove_mount("/mnt/aim-bind-fuse/Android"));assert!(crate::vfs::remove_mount("/mnt/aim-bind-fuse"));assert!(crate::vfs::remove_mount("/mnt/aim-bind-lower"));
}

#[test]
fn fuse_transferred_mount_device_daemon_death_aborts_pending_lookup() {
    let mut connection = Connection::new();
    initialize(&connection);
    // Binder passes a duplicate of vold's device to the original daemon.
    let daemon = unsafe { libc::dup(connection.fd) };
    assert!(daemon >= 0);
    // Release the sender before the first bind lookup, as std::move(fd)
    // does in original EmulatedVolume::doMount.
    unsafe { libc::close(connection.fd); }
    connection.fd = -1;
    let client = Client::from_key(&connection.key).unwrap();
    let (sent, received) = mpsc::channel();
    let pending = std::thread::spawn(move || {
        sent.send(client.request(1, 1, b"Android\0", 0, 0, 1).err()).unwrap();
    });
    let packet = next(daemon);
    assert_eq!(u32::from_le_bytes(packet[4..8].try_into().unwrap()), 1);
    assert!(received.try_recv().is_err());
    // A killed original daemon closes its duplicate without replying.
    unsafe { libc::close(daemon); }
    assert_eq!(received.recv_timeout(Duration::from_secs(5)).unwrap(), Some(107));
    pending.join().unwrap();
}

#[test]
fn vfs_path_walk_releases_lookup_references_on_success_and_errors() {
 let (_guard, root) = crate::vfs::test_view();
 let connection = Connection::new(); initialize(&connection);
 let mount = "/mnt/aim-lookup-reference";
 let anchor = root.join("lookup-reference-anchor"); fs::create_dir_all(&anchor).unwrap();
 let options = super::super::fuse_mount::parse(b"fd=17,rootmode=40000,user_id=0,group_id=0,allow_other,").unwrap();
 crate::vfs::add_fuse_mount(mount, connection.key.transport().to_owned(), anchor, "fuse", &options, false).unwrap();
 let device = connection.fd;
 let daemon = std::thread::spawn(move || {
  let mut refs = std::collections::BTreeMap::<u64,i64>::new();
  let mut lookups = 0; let mut forgets = 0;
  loop {
   let packet = next(device); let opcode = u32::from_le_bytes(packet[4..8].try_into().unwrap());
   let node = u64::from_le_bytes(packet[16..24].try_into().unwrap());
   match opcode {
    1 => {
     let name = &packet[40..];
     let inode = match name { b"file\0" => 10, b"denied\0" => 11, b"link\0" => 12, _ => panic!("unexpected lookup {name:?}") };
     *refs.entry(inode).or_default() += 1; lookups += 1;
     let mut entry = vec![0;128]; entry[..8].copy_from_slice(&inode.to_le_bytes());
     respond(device, &packet, 0, &entry);
    }
    3 => {
     if node == 11 { respond(device, &packet, -crate::errno::EACCES, &[]); continue; }
     let mut attr = vec![0;104]; attr[16..24].copy_from_slice(&node.to_le_bytes());
     let mode:u32 = if node == 1 {0o040755} else if node == 12 {0o120777} else {0o100644};
     attr[76..80].copy_from_slice(&mode.to_le_bytes()); respond(device, &packet, 0, &attr);
    }
    5 => { assert_eq!(node,12); assert_eq!(refs.get(&node),Some(&1)); respond(device, &packet, -crate::errno::EINVAL, &[]); }
    FORGET => {
     let count = u64::from_le_bytes(packet[40..48].try_into().unwrap());
     *refs.entry(node).or_default() -= count as i64; forgets += count;
     assert!(refs[&node] >= 0, "duplicate FORGET for {node}");
    }
    17 => { respond(device, &packet, 0, &[0;80]); break; }
    _ => panic!("unexpected FUSE opcode {opcode}"),
   }
  }
  assert_eq!(lookups,3); assert_eq!(forgets,3); assert!(refs.values().all(|count|*count==0));
 });
 assert!(crate::vfs::resolve(crate::vfs::LINUX_AT_FDCWD,format!("{mount}/file").as_bytes(),true).is_ok());
 assert_eq!(crate::vfs::resolve(crate::vfs::LINUX_AT_FDCWD,format!("{mount}/denied").as_bytes(),true).err(),Some(crate::errno::EACCES));
 assert_eq!(crate::vfs::resolve(crate::vfs::LINUX_AT_FDCWD,format!("{mount}/link").as_bytes(),true).err(),Some(crate::errno::EINVAL));
 // A following real request makes the daemon drain all preceding no-reply FORGETs.
 Client::from_key(&connection.key).unwrap().request(17,1,&[],0,0,1).unwrap();
 daemon.join().unwrap(); assert!(crate::vfs::remove_mount(mount));
}
