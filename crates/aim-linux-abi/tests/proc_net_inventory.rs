//! Real sockets through the Linux ABI, with no image or running guest input.
//! #1144: supported inventories are read through stat/open/read, while an
//! unknowable timer/probe state must fail instead of publishing invented rows.
use aim_linux_abi::{context, sys, vfs};
use std::ffi::CString;
use std::time::{Duration, Instant};

fn call(nr: u64, args: [u64; 6]) -> i64 {
    let ctx = context::current_ctx();
    unsafe { (&mut (*ctx).x)[..6].copy_from_slice(&args); (*ctx).x[8] = nr; sys::dispatch(&mut *ctx); (*ctx).x[0] as i64 }
}
struct Socket(i32);
impl Drop for Socket { fn drop(&mut self) { assert_eq!(call(57,[self.0 as u64,0,0,0,0,0]),0); } }
fn socket(family: u64, ty: u64) -> Socket {
    let fd = call(198,[family,ty,0,0,0,0]); assert!(fd>=0,"socket errno {fd}"); Socket(fd as i32)
}
fn address(family: u64, port: u16) -> Vec<u8> {
    let mut value = vec![0; if family==2 {16} else {28}];
    value[..2].copy_from_slice(&(family as u16).to_ne_bytes()); value[2..4].copy_from_slice(&port.to_be_bytes());
    if family==2 { value[4..8].copy_from_slice(&[127,0,0,1]); } else { value[23]=1; } value
}
fn bind(socket: &Socket, family: u64) -> Vec<u8> {
    let mut value = address(family,0);
    assert_eq!(call(200,[socket.0 as u64,value.as_ptr() as u64,value.len() as u64,0,0,0]),0);
    let mut size = value.len() as u32;
    assert_eq!(call(204,[socket.0 as u64,value.as_mut_ptr() as u64,(&mut size as *mut u32) as u64,0,0,0]),0); value
}
fn read_table(kind: &str) -> Result<String,i64> {
    let path = CString::new(format!("/proc/net/{kind}")).unwrap();
    let mut stat = [0u8;128];
    assert_eq!(call(79,[vfs::LINUX_AT_FDCWD as u64,path.as_ptr() as u64,stat.as_mut_ptr() as u64,0,0,0]),0);
    assert_eq!(u32::from_ne_bytes(stat[16..20].try_into().unwrap()) & 0o170000,0o100000);
    let fd = call(56,[vfs::LINUX_AT_FDCWD as u64,path.as_ptr() as u64,0,0,0,0]); if fd<0 { return Err(fd); }
    let mut bytes = Vec::new();let mut buffer=[0u8;4096];
    loop { let size=call(63,[fd as u64,buffer.as_mut_ptr() as u64,buffer.len() as u64,0,0,0]);assert!(size>=0);if size==0 {break;}bytes.extend_from_slice(&buffer[..size as usize]); }
    assert_eq!(call(57,[fd as u64,0,0,0,0,0]),0);Ok(String::from_utf8(bytes).unwrap())
}
fn supported_table(kind: &str) -> String {
    let until = Instant::now()+Duration::from_secs(2);
    loop { match read_table(kind) { Ok(table)=>return table,Err(-95) if Instant::now()<until=>std::thread::yield_now(),Err(error)=>panic!("table {kind}: {error}") } }
}
fn endpoint(family: u64, addr: &[u8]) -> String {
    let ip = if family==2 {"0100007F"} else {"00000000000000000000000001000000"};
    format!("{ip}:{:04X}",u16::from_be_bytes([addr[2],addr[3]]))
}
fn traffic(sender:&Socket,receiver:&Socket) {
    let sent=b"owned";assert_eq!(call(206,[sender.0 as u64,sent.as_ptr() as u64,sent.len() as u64,0,0,0]),5);
    let mut received=[0u8;5];assert_eq!(call(207,[receiver.0 as u64,received.as_mut_ptr() as u64,5,0,0,0]),5);assert_eq!(&received,sent);
}
#[test]
fn isolated_registered_ipv4_ipv6_idle_active_and_unknown_probe_states() {
    context::init_thread();
    let directory=std::env::temp_dir().join(format!("aim-procnet-integration-{}",std::process::id()));
    std::fs::create_dir(&directory).unwrap();vfs::init(&directory,None).unwrap();
    let uid=call(175,[0;6]);
    for family in [2,10] {
        let suffix=if family==2 {""} else {"6"};let tcp=format!("tcp{suffix}");let udp=format!("udp{suffix}");
        let listener=socket(family,1);let local=bind(&listener,family);
        assert_eq!(call(201,[listener.0 as u64,2,0,0,0,0]),0);
        let idle=supported_table(&tcp);let row=idle.lines().find(|row|row.contains(&endpoint(family,&local))).unwrap();
        let fields=row.split_whitespace().collect::<Vec<_>>();assert_eq!(fields[3],"0A");assert_eq!(fields[7].parse::<i64>().unwrap(),uid);
        let client=socket(family,1);assert_eq!(call(203,[client.0 as u64,local.as_ptr() as u64,local.len() as u64,0,0,0]),0);
        let accepted=call(242,[listener.0 as u64,0,0,0,0,0]);assert!(accepted>=0);let accepted=Socket(accepted as i32);
        traffic(&client,&accepted);
        let active=supported_table(&tcp);assert!(active.lines().any(|row|row.contains(&endpoint(family,&local))&&row.split_whitespace().nth(3)==Some("01")));
        let udp_receiver=socket(family,2);let udp_addr=bind(&udp_receiver,family);let udp_sender=socket(family,2);
        assert_eq!(call(203,[udp_sender.0 as u64,udp_addr.as_ptr() as u64,udp_addr.len() as u64,0,0,0]),0);traffic(&udp_sender,&udp_receiver);
        assert!(supported_table(&udp).contains(&endpoint(family,&udp_addr)));
        let enabled=1i32;assert_eq!(call(208,[accepted.0 as u64,1,9,(&enabled as *const i32) as u64,4,0]),0);
        assert_eq!(read_table(&tcp),Err(-95)); // Known missing exact keepalive producer, not empty success.
        assert!(read_table(&udp).is_ok()); // The unrelated TCP limitation must not hide UDP inventory.
    }
    std::fs::remove_dir(directory).unwrap();
}
