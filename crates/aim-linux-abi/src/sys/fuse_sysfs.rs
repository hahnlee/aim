//! Linux fusectl exposes real mounted connections, not unmounted device opens.
use crate::{errno::{Errno,EINVAL,EIO,ENOENT},vfs};
use super::{fuse::{self,SessionKey,Client},procfs::Node,dir::{self,Entry}};
use std::{fs,path::PathBuf,os::unix::fs::OpenOptionsExt,io::Write};
const ROOT:&str="/sys/fs/fuse/connections";
fn directory()->Result<PathBuf,Errno>{Ok(vfs::runtime_dir().ok_or(crate::errno::ENODEV)?.join("fuse-connections"))}
pub fn mounted(key:&SessionKey)->Result<u64,Errno>{
    let directory=directory()?;fs::create_dir_all(&directory).map_err(|_|EIO)?;
    let name=key.transport().parent().and_then(|path|path.file_name()).and_then(|name|name.to_str()).ok_or(EINVAL)?;
    let number=u64::from_str_radix(name.strip_prefix("af-").ok_or(EINVAL)?,16).map_err(|_|EINVAL)?;
    let file=directory.join(number.to_string());
    match fs::OpenOptions::new().create_new(true).write(true).mode(0o600).custom_flags(libc::O_NOFOLLOW|libc::O_CLOEXEC).open(&file){
        Ok(mut output)=>{output.write_all(key.transport().as_os_str().as_encoded_bytes()).map_err(|_|EIO)?;output.sync_all().map_err(|_|EIO)?;},
        Err(error)if error.kind()==std::io::ErrorKind::AlreadyExists=>{if fs::read(&file).map_err(|_|EIO)?!=key.transport().as_os_str().as_encoded_bytes(){return Err(EINVAL);}},
        Err(_)=>return Err(EIO),
    }Ok(number)
}
fn active()->Result<Vec<(u64,SessionKey)>,Errno>{
    let directory=directory()?;let entries=match fs::read_dir(&directory){Ok(entries)=>entries,Err(error)if error.kind()==std::io::ErrorKind::NotFound=>return Ok(Vec::new()),Err(_)=>return Err(EIO)};
    let mut result=Vec::new();
    for entry in entries{let entry=entry.map_err(|_|EIO)?;let id=entry.file_name().to_str().and_then(|name|name.parse().ok()).ok_or(EIO)?;
        let key=SessionKey::from_transport(PathBuf::from(String::from_utf8(fs::read(entry.path()).map_err(|_|EIO)?).map_err(|_|EIO)?));
        if Client::from_key(&key).is_err(){let _=fs::remove_file(entry.path());continue;}result.push((id,key));
    }result.sort_by_key(|(id,_)|*id);Ok(result)
}
fn selected(guest:&str)->Result<SessionKey,Errno>{let rest=guest.strip_prefix(ROOT).ok_or(ENOENT)?.trim_start_matches('/');let id=rest.split('/').next().and_then(|name|name.parse::<u64>().ok()).ok_or(ENOENT)?;active()?.into_iter().find(|(number,_)|*number==id).map(|(_,key)|key).ok_or(ENOENT)}
pub fn node(guest:&str)->Option<Result<Node,Errno>>{
    if guest=="/sys/fs/fuse"{return Some(Ok(Node::Dir(vec![Entry::new(1,dir::DT_DIR,"connections")])));}
    if guest==ROOT{return Some(active().map(|connections|Node::Dir(connections.into_iter().map(|(id,_)|Entry::new(id,dir::DT_DIR,id.to_string())).collect())));}
    if !guest.starts_with(&format!("{ROOT}/")){return None;}
    Some((||{selected(guest)?;let rest=guest.strip_prefix(ROOT).unwrap().trim_start_matches('/');if !rest.contains('/'){return Ok(Node::Dir(vec![Entry::new(1,dir::DT_REG,"abort")]));}
        if rest.split_once('/').unwrap().1=="abort"{Ok(Node::File(Vec::new()))}else{Err(ENOENT)}
    })())
}
pub fn open(guest:&str,flags:u64)->Option<i64>{
    if !guest.starts_with(&format!("{ROOT}/"))||!guest.ends_with("/abort"){return None;}
    let key=match selected(guest){Ok(key)=>key,Err(error)=>return Some(-(error as i64))};
    if flags&3==0{return Some(-(crate::errno::EACCES as i64));}
    if super::cred::current().uid[3]!=0{return Some(-(crate::errno::EACCES as i64));}
    Some(super::knob::open(&[],flags&0o2000000!=0,move|_bytes|{fuse::abort(&key)?;Ok(None)}))
}
