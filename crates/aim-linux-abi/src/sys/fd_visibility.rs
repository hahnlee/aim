//! Publication of guest descriptors, distinct from kernel-owned host slots (#226).
//! The fast path is enabled only for an explicitly published plain descriptor.
use crate::errno::{Errno,EBADF};
use std::{collections::HashMap,sync::{Mutex,LazyLock,atomic::{AtomicU8,Ordering}}};
const CAPACITY:usize=65536;
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
#[repr(u8)]
pub enum Role{Unknown=0,Plain=1,Typed=2,Hidden=3}
static ROLES:[AtomicU8;CAPACITY]=[const{AtomicU8::new(0)};CAPACITY];
static HIGH:LazyLock<Mutex<HashMap<i32,Role>>>=LazyLock::new(Default::default);
fn decode(value:u8)->Role{match value{1=>Role::Plain,2=>Role::Typed,3=>Role::Hidden,_=>Role::Unknown}}
pub fn role(fd:i32)->Role{
 if fd<0{return Role::Unknown;}
 ROLES.get(fd as usize).map(|value|decode(value.load(Ordering::Acquire))).unwrap_or_else(||HIGH.lock().unwrap().get(&fd).copied().unwrap_or(Role::Unknown))
}
pub fn require(fd:i32)->Result<(),Errno>{match role(fd){Role::Plain|Role::Typed=>Ok(()),_=>Err(EBADF)}}
pub fn publish(fd:i32,typed:bool)->Result<(),Errno>{
 if fd<0{return Err(EBADF);}let next=if typed{Role::Typed}else{Role::Plain};
 if let Some(slot)=ROLES.get(fd as usize){
  let mut old=slot.load(Ordering::Acquire);
  loop{if decode(old)==Role::Hidden{return Err(EBADF);}match slot.compare_exchange_weak(old,next as u8,Ordering::AcqRel,Ordering::Acquire){Ok(_)=>return Ok(()),Err(value)=>old=value}}
 }else{let mut high=HIGH.lock().unwrap();if high.get(&fd)==Some(&Role::Hidden){return Err(EBADF);}high.insert(fd,next);Ok(())}
}
pub fn hide(fd:i32)->Result<(),Errno>{
 if fd<0{return Err(EBADF);}
 if let Some(slot)=ROLES.get(fd as usize){
  match slot.compare_exchange(Role::Unknown as u8,Role::Hidden as u8,Ordering::AcqRel,Ordering::Acquire){Ok(_)=>Ok(()),Err(value)if decode(value)==Role::Hidden=>Ok(()),Err(_)=>Err(EBADF)}
 }else{let mut high=HIGH.lock().unwrap();match high.get(&fd){None|Some(Role::Hidden)=>{high.insert(fd,Role::Hidden);Ok(())},_=>Err(EBADF)}}
}
/// A close/replacement owner withdraws visibility before closing the actual FD.
pub fn withdraw(fd:i32)->Result<Role,Errno>{
 if fd<0{return Err(EBADF);}
 if let Some(slot)=ROLES.get(fd as usize){
  let mut old=slot.load(Ordering::Acquire);
  loop{match decode(old){Role::Unknown|Role::Hidden=>return Err(EBADF),_=>{}}
   match slot.compare_exchange_weak(old,Role::Unknown as u8,Ordering::AcqRel,Ordering::Acquire){Ok(_)=>return Ok(decode(old)),Err(value)=>old=value}
  }
 }else{let mut high=HIGH.lock().unwrap();match high.get(&fd).copied(){Some(Role::Plain|Role::Typed)=>Ok(high.remove(&fd).unwrap()),_=>Err(EBADF)}}
}
/// Called after actual private descriptor close, never before its slot is gone.
pub fn private_closed(fd:i32){
 if let Some(slot)=ROLES.get(fd as usize){let _=slot.compare_exchange(Role::Hidden as u8,Role::Unknown as u8,Ordering::AcqRel,Ordering::Acquire);}
 else{let mut high=HIGH.lock().unwrap();if high.get(&fd)==Some(&Role::Hidden){high.remove(&fd);}}
}
pub fn visible()->Vec<i32>{
 let mut fds=ROLES.iter().enumerate().filter_map(|(fd,value)|matches!(decode(value.load(Ordering::Acquire)),Role::Plain|Role::Typed).then_some(fd as i32)).collect::<Vec<_>>();
 fds.extend(HIGH.lock().unwrap().iter().filter_map(|(fd,role)|matches!(role,Role::Plain|Role::Typed).then_some(*fd)));fds
}
#[cfg(test)]
mod tests{
 use super::*;use std::{fs::File,os::fd::AsRawFd};
 #[test]
 fn real_host_slot_is_not_public_until_owner_publishes_it(){
  if super::super::fdtab::isolated_kernel_test("sys::fd_visibility::tests::real_host_slot_is_not_public_until_owner_publishes_it"){return;}
  let file=File::open("/dev/null").unwrap();let fd=file.as_raw_fd();assert_eq!(role(fd),Role::Unknown);assert_eq!(require(fd),Err(EBADF));
  hide(fd).unwrap();assert_eq!(publish(fd,false),Err(EBADF));assert_eq!(withdraw(fd),Err(EBADF));
  drop(file);private_closed(fd);assert_eq!(role(fd),Role::Unknown);
  let file=File::open("/dev/null").unwrap();let fd=file.as_raw_fd();publish(fd,false).unwrap();assert_eq!(require(fd),Ok(()));assert_eq!(withdraw(fd).unwrap(),Role::Plain);assert_eq!(require(fd),Err(EBADF));drop(file);
 }
 #[test]
 fn close_reuse_and_high_slots_require_fresh_publication(){
  let old=File::open("/dev/null").unwrap();let fd=old.as_raw_fd();publish(fd,true).unwrap();assert!(visible().contains(&fd));assert_eq!(withdraw(fd).unwrap(),Role::Typed);drop(old);
  let other=File::open("/dev/zero").unwrap();let slot=unsafe{libc::fcntl(other.as_raw_fd(),libc::F_DUPFD_CLOEXEC,CAPACITY as i32)};
  if slot>=0{assert_eq!(role(slot),Role::Unknown);publish(slot,false).unwrap();assert_eq!(require(slot),Ok(()));withdraw(slot).unwrap();assert_eq!(unsafe{libc::close(slot)},0);}
  assert_eq!(require(-1),Err(EBADF));assert_eq!(publish(-1,false),Err(EBADF));
 }
}
