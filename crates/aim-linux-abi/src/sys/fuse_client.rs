//! Linux FUSE filesystem requests over the device owner's durable broker.
use crate::{errno::{Errno,EIO,EINVAL},vfs::FuseRoute};
use super::fuse::{Client,SessionKey};
fn request(route:&FuseRoute,opcode:u32,node:u64,data:&[u8])->Result<Vec<u8>,Errno>{
    let identity=super::cred::current();
    if !route.allow_other&&identity.uid[1]!=route.uid&&identity.uid[1]!=0{return Err(crate::errno::EACCES);}
    if route.read_only&&matches!(opcode,4|6|8|9|10|11|12|13|16|21|24|35|43|45){return Err(crate::errno::EROFS);}
    Client::from_key(&SessionKey::from_transport(route.session.clone()))?.request(opcode,node,data,identity.uid[3],identity.gid[3],super::process::getpid() as u32).map(|reply|reply.body)
}
fn u64_at(data:&[u8],at:usize)->Result<u64,Errno>{Ok(u64::from_le_bytes(data.get(at..at+8).ok_or(EIO)?.try_into().unwrap()))}
fn u32_at(data:&[u8],at:usize)->Result<u32,Errno>{Ok(u32::from_le_bytes(data.get(at..at+4).ok_or(EIO)?.try_into().unwrap()))}
fn current_umask()->u32{super::pstate::current_umask()}
fn put64(data:&mut Vec<u8>,value:u64){data.extend(value.to_le_bytes());}
fn put32(data:&mut Vec<u8>,value:u32){data.extend(value.to_le_bytes());}
pub fn forget(route:&FuseRoute,node:u64,count:u64)->Result<(),Errno>{
    if node==1{return Ok(());}let identity=super::cred::current();
    Client::from_key(&SessionKey::from_transport(route.session.clone()))?.request_no_reply(2,node,&count.to_le_bytes(),identity.uid[3],identity.gid[3],super::process::getpid() as u32)
}
pub fn lookup(route:&FuseRoute)->Result<u64,Errno>{
    let mut node=1;
    for component in route.relative.split('/').filter(|name|!name.is_empty()){
        if component=="."||component==".."||component.as_bytes().contains(&0){return Err(EINVAL);}
        if route.default_permissions{
            let attributes=getattr(route,node,None)?;let mode=u32_at(&attributes,60)?;let identity=super::cred::current();
            if mode&0o170000!=0o040000{return Err(crate::errno::ENOTDIR);}
            let uid=u32_at(&attributes,68)?;let gid=u32_at(&attributes,72)?;let shift=if identity.uid[3]==uid{6}else if identity.gid[3]==gid||identity.groups.contains(&gid){3}else{0};
            if identity.uid[3]!=0&&(mode>>shift)&1==0{return Err(crate::errno::EACCES);}
        }
        let mut name=component.as_bytes().to_vec();name.push(0);let reply=match request(route,1,node,&name){Ok(reply)=>reply,Err(error)=>{eprintln!("FUSE LOOKUP failed session={} relative={} parent={} component={} uid={} gid={} pid={} errno={}",route.session.display(),route.relative,node,component,super::cred::current().uid[3],super::cred::current().gid[3],super::process::getpid(),error);if node!=1{let _=forget(route,node,1);}return Err(error);}};
        let next=u64_at(&reply,0)?;if next==0{return Err(crate::errno::ENOENT);}
        if node!=1{forget(route,node,1)?;}node=next;
    }Ok(node)
}
pub fn getattr(route:&FuseRoute,node:u64,fh:Option<u64>)->Result<Vec<u8>,Errno>{
    let mut data=Vec::new();put32(&mut data,u32::from(fh.is_some()));put32(&mut data,0);put64(&mut data,fh.unwrap_or(0));
    let reply=request(route,3,node,&data)?;if reply.len()<104{return Err(EIO);}Ok(reply[16..104].to_vec())
}
pub(crate) struct LookupGuard{route:FuseRoute,node:u64,keep:bool}
impl LookupGuard {
    pub(crate) fn transient(route: &FuseRoute, node: u64) -> Self {
        Self { route: route.clone(), node, keep: false }
    }
}
impl Drop for LookupGuard{fn drop(&mut self){if !self.keep{if let Err(error)=forget(&self.route,self.node,1){eprintln!("FUSE transient FORGET failed: {error}");}}}}
pub struct Open {pub guest:String,pub route:FuseRoute,pub node:u64,pub fh:u64,pub flags:u32,pub open_flags:u32,pub broker_owned:bool,pub directory:bool,pub offset:std::sync::Mutex<u64>}
impl Open{
    pub fn open(route:FuseRoute,flags:u32)->Result<Self,Errno>{let node=lookup(&route)?;let mut lookup=LookupGuard{route:route.clone(),node,keep:false};let attr=getattr(&route,node,None)?;let directory=u32_at(&attr,60)?&0o170000==0o040000;
        if route.default_permissions{
            let mode=u32_at(&attr,60)?;let uid=u32_at(&attr,68)?;let gid=u32_at(&attr,72)?;let identity=super::cred::current();
            let requested=match flags&3{0=>4,1=>2,_=>6};
            let shift=if identity.uid[3]==uid{6}else if identity.gid[3]==gid||identity.groups.contains(&gid){3}else{0};
            if identity.uid[3]!=0&&((mode>>shift)&requested)!=requested{return Err(crate::errno::EACCES);}
        }
        if flags&0o200000!=0&&!directory{return Err(crate::errno::ENOTDIR);}
        if flags&0o10000000!=0{lookup.keep=true;return Ok(Self{guest:String::new(),route,node,fh:0,flags,open_flags:0,broker_owned:false,directory,offset:std::sync::Mutex::new(0)});}
        let mut data=Vec::new();put32(&mut data,flags);put32(&mut data,0);let reply=request(&route,if directory{27}else{14},node,&data)?;
        let fh=u64_at(&reply,0)?;let open_flags=u32_at(&reply,8)?;lookup.keep=true;
        Ok(Self{guest:String::new(),route,node,fh,flags,open_flags,broker_owned:false,directory,offset:std::sync::Mutex::new(0)})}
    pub fn read(&self,offset:u64,size:u32)->Result<Vec<u8>,Errno>{let mut data=Vec::new();put64(&mut data,self.fh);put64(&mut data,offset);put32(&mut data,size);put32(&mut data,0);put64(&mut data,0);put32(&mut data,self.flags);put32(&mut data,0);
        let bytes=request(&self.route,if self.directory{28}else{15},self.node,&data)?;if bytes.len()>size as usize{return Err(EIO);}Ok(bytes)}
    pub fn write(&self,offset:u64,bytes:&[u8])->Result<u32,Errno>{let mut data=Vec::new();put64(&mut data,self.fh);put64(&mut data,offset);put32(&mut data,u32::try_from(bytes.len()).map_err(|_|EINVAL)?);put32(&mut data,0);put64(&mut data,0);put32(&mut data,self.flags);put32(&mut data,0);data.extend(bytes);
        let size=u32_at(&request(&self.route,16,self.node,&data)?,0)?;if size as usize>bytes.len(){return Err(EIO);}Ok(size)}
    pub fn fsync(&self,datasync:bool)->Result<(),Errno>{let mut data=Vec::new();put64(&mut data,self.fh);put32(&mut data,u32::from(datasync));put32(&mut data,0);request(&self.route,if self.directory{30}else{20},self.node,&data).map(|_|())}
}
impl Drop for Open{fn drop(&mut self){if self.broker_owned{return;}if self.flags&0o10000000!=0{let _=forget(&self.route,self.node,1);return;}let mut data=Vec::new();put64(&mut data,self.fh);put32(&mut data,self.flags);put32(&mut data,0);put64(&mut data,0);if let Err(error)=request(&self.route,if self.directory{29}else{18},self.node,&data){eprintln!("FUSE release failed: {error}");}let _=forget(&self.route,self.node,1);}}

static FILES:std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<i32,std::sync::Arc<Open>>>>=std::sync::LazyLock::new(Default::default);
pub fn adopt(fd:i32)->Result<bool,Errno>{
    if super::fuse::marker(fd)!=Some(super::fuse::FILE_MARKER){return Ok(false);}
    if FILES.lock().unwrap().contains_key(&fd){return Ok(true);}
    let description=super::fuse::description(fd)?;
    crate::vfs::refresh_fuse_mounts()?;
    let route=FuseRoute{session:description.key.transport().to_path_buf(),relative:description.relative,
        uid:description.policy.uid,gid:description.policy.gid,allow_other:description.policy.allow_other,
        default_permissions:description.policy.default_permissions,read_only:description.policy.read_only};
    let file=Open{guest:description.guest,route,node:description.node,fh:description.fh,flags:description.flags,open_flags:description.open_flags,broker_owned:true,directory:description.directory,offset:std::sync::Mutex::new(0)};
    let file=std::sync::Arc::new(file);super::fuse_cache::register_file(&file)?;FILES.lock().unwrap().insert(fd,file);if let Some(slow)=super::fdtab::SLOW.get(fd as usize){slow.store(1,std::sync::atomic::Ordering::Relaxed);}Ok(true)
}
pub fn get(fd:i32)->Option<std::sync::Arc<Open>>{FILES.lock().unwrap().get(&fd).cloned()}
pub fn inherited_error(fd:i32)->Option<Errno>{if super::fuse::marker(fd)==Some(super::fuse::FILE_MARKER)&&get(fd).is_none(){return adopt(fd).err();}None}
pub fn close(fd:i32){let removed=FILES.lock().unwrap().remove(&fd);if removed.is_some(){if let Some(slow)=super::fdtab::SLOW.get(fd as usize){slow.store(0,std::sync::atomic::Ordering::Relaxed);}}drop(removed);}
pub fn dup(old:i32,new:i32){let mut files=FILES.lock().unwrap();let removed=files.remove(&new);if let Some(file)=files.get(&old).cloned(){files.insert(new,file);if let Some(slow)=super::fdtab::SLOW.get(new as usize){slow.store(1,std::sync::atomic::Ordering::Relaxed);}}drop(files);drop(removed);}
pub fn open_fd(route:FuseRoute,guest:String,flags:u32,mode:u32)->Result<i32,Errno>{
    let mut opened=match Open::open(route.clone(),flags){
        Ok(opened)=>{if flags&0o200!=0&&flags&0o100!=0{return Err(crate::errno::EEXIST);}if flags&0o400000!=0&&stat(&opened.route,Some(opened.node),None)?.st_mode&libc::S_IFMT==libc::S_IFLNK{return Err(crate::errno::ELOOP);}opened},
        Err(error)if error==crate::errno::ENOENT&&flags&0o100!=0=>{
            let(parent,name)=route.relative.rsplit_once('/').unwrap_or(("",route.relative.as_str()));
            let mut parent_route=route.clone();parent_route.relative=parent.into();let node=lookup(&parent_route)?;
            let mut data=Vec::new();put32(&mut data,flags);put32(&mut data,mode);put32(&mut data,current_umask());put32(&mut data,0);data.extend(name.as_bytes());data.push(0);
            let reply=request(&route,35,node,&data)?;
            Open{guest:String::new(),route,node:u64_at(&reply,0)?,fh:u64_at(&reply,128)?,flags,open_flags:u32_at(&reply,136)?,broker_owned:false,directory:false,offset:std::sync::Mutex::new(0)}
        }
        Err(error)=>return Err(error),
    };
    opened.guest=guest;
    if flags&0o1000!=0{
        let mut data=vec![0u8;88];data[0..4].copy_from_slice(&(1u32<<3|1u32<<6).to_le_bytes());data[8..16].copy_from_slice(&opened.fh.to_le_bytes());
        request(&opened.route,4,opened.node,&data)?;
    }
    let identity=super::cred::current();
    let fd=super::fuse::open_description(&SessionKey::from_transport(opened.route.session.clone()),opened.node,opened.fh,opened.flags,opened.directory,&opened.route.relative,&opened.guest,opened.open_flags,&super::fuse::MountPolicy{uid:opened.route.uid,gid:opened.route.gid,allow_other:opened.route.allow_other,default_permissions:opened.route.default_permissions,read_only:opened.route.read_only},identity.uid[3],identity.gid[3],super::process::getpid() as u32)?;
    opened.broker_owned=true;
    let file=std::sync::Arc::new(opened);
    if let Err(error)=super::fuse_cache::register_file(&file){unsafe{libc::close(fd);}return Err(error);}
    if fd<0{return Err(EIO);}FILES.lock().unwrap().insert(fd,file);
    if let Some(slow)=super::fdtab::SLOW.get(fd as usize){slow.store(1,std::sync::atomic::Ordering::Relaxed);}Ok(fd)
}
pub fn rw(fd:i32,iov:&[libc::iovec],position:Option<i64>,write:bool)->Option<i64>{
    let file=get(fd)?;if file.flags&0o10000000!=0{return Some(-(crate::errno::EBADF as i64));}if file.directory{return Some(-(crate::errno::EISDIR as i64));}
    if write&&file.flags&3==0||!write&&file.flags&3==1{return Some(-(crate::errno::EBADF as i64));}
    if position.is_some_and(|offset|offset<0){return Some(-(EINVAL as i64));}
    if let Err(error)=super::fuse_cache::flush_file(&file){return Some(-(error as i64));}
    let mut total=0i64;let mut explicit=position.map(|value|value as u64);
    for vector in iov{let size=vector.iov_len.min(128*1024);if size==0{continue;}
        let data=if write{unsafe{std::slice::from_raw_parts(vector.iov_base.cast::<u8>(),size)}}else{&[]};
        match super::fuse::description_io_at(fd,write,explicit,data,size){
            Ok(reply)=>{let bytes=reply.body;let count=if write{match u32_at(&bytes,0){Ok(count)=>count as usize,Err(error)=>return Some(-(error as i64))}}else{if bytes.len()>size{return Some(-(EIO as i64));}unsafe{std::ptr::copy_nonoverlapping(bytes.as_ptr(),vector.iov_base.cast::<u8>(),bytes.len());}bytes.len()};
                if write{if let Err(error)=super::fuse_cache::invalidate_range(&file,reply.offset,count as u64){return Some(-(error as i64));}}
                total+=count as i64;if let Some(offset)=&mut explicit{*offset+=count as u64;}if count<size{break;}
            },Err(error)=>return Some(if total>0{total}else{-(error as i64)})
        }
    }Some(total)
}

pub fn seek(fd:i32,offset:i64,whence:u32)->Option<i64>{let file=get(fd)?;let base=match whence{0=>0,1=>match super::fuse::offset(fd,None){Ok(offset)=>offset,Err(error)=>return Some(-(error as i64))},2=>match getattr(&file.route,file.node,Some(file.fh)).and_then(|attr|u64_at(&attr,8)){Ok(size)=>size,Err(error)=>return Some(-(error as i64))},_=>return Some(-(EINVAL as i64))};let next=i128::from(base)+i128::from(offset);if !(0..=i64::MAX as i128).contains(&next){return Some(-(EINVAL as i64));}Some(super::fuse::offset(fd,Some(next as u64)).map(|_|next as i64).unwrap_or_else(|error|-(error as i64)))}


pub fn stat(route:&FuseRoute,node:Option<u64>,fh:Option<u64>)->Result<libc::stat,Errno>{
    let own_lookup=node.is_none();let node=match node{Some(node)=>node,None=>lookup(route)?};let attr=getattr(route,node,fh)?;if own_lookup{forget(route,node,1)?;}
    let mut stat:libc::stat=unsafe{std::mem::zeroed()};stat.st_dev=0x00f5;stat.st_ino=u64_at(&attr,0)?;stat.st_size=u64_at(&attr,8)? as i64;stat.st_blocks=u64_at(&attr,16)? as i64;
    stat.st_atime=u64_at(&attr,24)? as i64;stat.st_mtime=u64_at(&attr,32)? as i64;stat.st_ctime=u64_at(&attr,40)? as i64;
    stat.st_atime_nsec=u32_at(&attr,48)? as i64;stat.st_mtime_nsec=u32_at(&attr,52)? as i64;stat.st_ctime_nsec=u32_at(&attr,56)? as i64;
    stat.st_mode=u32_at(&attr,60)? as u16;stat.st_nlink=u32_at(&attr,64)? as u16;stat.st_uid=u32_at(&attr,68)?;stat.st_gid=u32_at(&attr,72)?;stat.st_rdev=u32_at(&attr,76)? as i32;stat.st_blksize=u32_at(&attr,80)? as i32;Ok(stat)
}
pub fn getdents(fd:i32,buffer:u64,count:usize)->Option<i64>{let file=get(fd)?;if !file.directory{return Some(-(crate::errno::ENOTDIR as i64));}
    let bytes=match super::fuse::description_io_at(fd,false,None,&[],count.min(128*1024)){Ok(reply)=>reply.body,Err(error)=>return Some(-(error as i64))};
    let(mut at,mut output)=(0,Vec::new());
    while at<bytes.len(){if bytes.len()-at<24{return Some(-(EIO as i64));}let ino=u64_at(&bytes,at).unwrap();let offset=u64_at(&bytes,at+8).unwrap();let length=u32_at(&bytes,at+16).unwrap() as usize;let kind=u32_at(&bytes,at+20).unwrap();let end=match (at+24).checked_add(length){Some(end)if end<=bytes.len()=>end,_=>return Some(-(EIO as i64))};
        let record=(19+length+1+7)&!7;if output.len()+record>count{if output.is_empty(){return Some(-(EINVAL as i64));}break;}
        put64(&mut output,ino);put64(&mut output,offset);output.extend((record as u16).to_le_bytes());output.push(kind as u8);output.extend(&bytes[at+24..end]);output.resize(output.len()+record-(19+length),0);at=(end+7)&!7;
    }
    unsafe{std::ptr::copy_nonoverlapping(output.as_ptr(),buffer as *mut u8,output.len());}Some(output.len() as i64)
}
fn parent(route:&FuseRoute)->Result<(u64,Vec<u8>),Errno>{let(base,name)=route.relative.rsplit_once('/').unwrap_or(("",route.relative.as_str()));if name.is_empty(){return Err(EINVAL);}let mut parent=route.clone();parent.relative=base.into();let mut name=name.as_bytes().to_vec();name.push(0);Ok((lookup(&parent)?,name))}
pub fn mkdir(route:&FuseRoute,mode:u32)->Result<(),Errno>{let(node,name)=parent(route)?;let _parent=LookupGuard{route:route.clone(),node,keep:false};let mut data=Vec::new();put32(&mut data,mode);put32(&mut data,current_umask());data.extend(name);request(route,9,node,&data).and_then(|reply|forget(route,u64_at(&reply,0)?,1))}
pub fn unlink(route:&FuseRoute,directory:bool)->Result<(),Errno>{let(node,name)=parent(route)?;let _parent=LookupGuard{route:route.clone(),node,keep:false};request(route,if directory{11}else{10},node,&name).map(|_|())}
pub(crate) fn readlink_node(route: &FuseRoute, node: u64) -> Result<Vec<u8>, Errno> {
    request(route, 5, node, &[])
}
pub fn readlink(route:&FuseRoute)->Result<Vec<u8>,Errno>{let node=lookup(route)?;let result=request(route,5,node,&[]);let release=forget(route,node,1);match result{Ok(bytes)=>{release?;Ok(bytes)},Err(error)=>Err(error)}}
pub fn access(route:&FuseRoute,mask:u32)->Result<(),Errno>{let node=lookup(route)?;let mut data=Vec::new();put32(&mut data,mask);put32(&mut data,0);let result=request(route,34,node,&data).map(|_|());let release=forget(route,node,1);result.and(release)}
pub fn rename(old:&FuseRoute,new:&FuseRoute,flags:u32)->Result<(),Errno>{if old.session!=new.session{return Err(crate::errno::EXDEV);}let(oldparent,oldname)=parent(old)?;let _old=LookupGuard{route:old.clone(),node:oldparent,keep:false};let(newparent,newname)=parent(new)?;let _new=LookupGuard{route:new.clone(),node:newparent,keep:false};let mut data=Vec::new();put64(&mut data,newparent);if flags!=0{put32(&mut data,flags);put32(&mut data,0);}data.extend(oldname);data.extend(newname);request(old,if flags==0{12}else{45},oldparent,&data).map(|_|())}
pub fn set_size(file:&Open,size:u64)->Result<(),Errno>{let mut data=vec![0u8;88];data[0..4].copy_from_slice(&(1u32<<3|1u32<<6).to_le_bytes());data[8..16].copy_from_slice(&file.fh.to_le_bytes());data[16..24].copy_from_slice(&size.to_le_bytes());request(&file.route,4,file.node,&data).map(|_|())}
pub fn setattr(route:&FuseRoute,node:Option<u64>,fh:Option<u64>,mode:Option<u32>,uid:Option<u32>,gid:Option<u32>,size:Option<u64>)->Result<(),Errno>{
    let own_lookup=node.is_none();let node=match node{Some(node)=>node,None=>lookup(route)?};let _lookup=LookupGuard{route:route.clone(),node,keep:!own_lookup};let mut data=vec![0u8;88];let mut valid=0u32;
    if let Some(mode)=mode{valid|=1;data[68..72].copy_from_slice(&mode.to_le_bytes());}
    if let Some(uid)=uid{valid|=2;data[76..80].copy_from_slice(&uid.to_le_bytes());}
    if let Some(gid)=gid{valid|=4;data[80..84].copy_from_slice(&gid.to_le_bytes());}
    if let Some(size)=size{valid|=8;data[16..24].copy_from_slice(&size.to_le_bytes());}
    if let Some(fh)=fh{valid|=64;data[8..16].copy_from_slice(&fh.to_le_bytes());}
    data[0..4].copy_from_slice(&valid.to_le_bytes());request(route,4,node,&data).map(|_|())
}
pub fn statfs(route:&FuseRoute)->Result<Vec<u8>,Errno>{request(route,17,1,&[])}
pub fn symlink(route:&FuseRoute,target:&[u8])->Result<(),Errno>{if target.contains(&0){return Err(EINVAL);}let(node,name)=parent(route)?;let _parent=LookupGuard{route:route.clone(),node,keep:false};let mut data=name;data.extend(target);data.push(0);request(route,6,node,&data).and_then(|reply|forget(route,u64_at(&reply,0)?,1))}
pub fn xattr(route:&FuseRoute,node:Option<u64>,opcode:u32,name:&[u8],value:&[u8],size:u32,flags:u32)->Result<Vec<u8>,Errno>{
    let own_lookup=node.is_none();let node=match node{Some(node)=>node,None=>lookup(route)?};let mut data=Vec::new();
    match opcode{21=>{put32(&mut data,value.len() as u32);put32(&mut data,flags);data.extend(name);data.push(0);data.extend(value);},22|23=>{put32(&mut data,size);put32(&mut data,0);if opcode==22{data.extend(name);data.push(0);}},24=>{data.extend(name);data.push(0);},_=>return Err(EINVAL)};
    let result=request(route,opcode,node,&data);if own_lookup{let release=forget(route,node,1);if result.is_ok(){release?;}}result
}
pub fn xattr_syscall(nr:u64,args:[u64;6])->Option<i64>{
    let fd=matches!(nr,7|10|13|16);let follow=!matches!(nr,6|9|12|15);
    let(route,node)=if fd{let file=get(args[0] as i32)?;(file.route.clone(),Some(file.node))}else{
        let path=unsafe{super::guest_cstr(args[0])};let resolved=match crate::vfs::resolve(crate::vfs::LINUX_AT_FDCWD,path,follow){Ok(resolved)=>resolved,Err(error)=>return Some(-(error as i64))};(crate::vfs::fuse_route(&resolved.guest)?,None)
    };
    let opcode=match nr{5..=7=>21,8..=10=>22,11..=13=>23,14..=16=>24,_=>return Some(-(EINVAL as i64))};
    let name=if opcode==23{&[][..]}else{unsafe{super::guest_cstr(args[1])}};
    if opcode!=23&&(name.is_empty()||name.len()>255){return Some(-(crate::errno::ERANGE as i64));}
    let(size,buffer)=if opcode==23{(args[2],args[1])}else{(args[3],args[2])};
    if size>65536{return Some(-(crate::errno::E2BIG as i64));}
    let value=if opcode==21&&size!=0{unsafe{std::slice::from_raw_parts(buffer as *const u8,size as usize)}}else{&[]};
    match xattr(&route,node,opcode,name,value,size as u32,args[4] as u32){
        Err(error)=>Some(-(error as i64)),Ok(bytes)=>{
            if opcode==21||opcode==24{return Some(0);}
            if size==0{return Some(match u32_at(&bytes,0){Ok(size)=>size as i64,Err(error)=>-(error as i64)});}
            if bytes.len()>size as usize{return Some(-(crate::errno::ERANGE as i64));}
            unsafe{std::ptr::copy_nonoverlapping(bytes.as_ptr(),buffer as *mut u8,bytes.len());}Some(bytes.len() as i64)
        }
    }
}
pub fn times(route:&FuseRoute,node:Option<u64>,fh:Option<u64>,times:&[libc::timespec;2])->Result<(),Errno>{
    let own_lookup=node.is_none();let node=match node{Some(node)=>node,None=>lookup(route)?};let _lookup=LookupGuard{route:route.clone(),node,keep:!own_lookup};let mut data=vec![0u8;88];let mut valid=0u32;
    for(index,time)in times.iter().enumerate(){if time.tv_nsec==libc::UTIME_OMIT{continue;}let(bit,now,sec,nsec)=if index==0{(16,128,32,56)}else{(32,256,40,60)};valid|=bit;
        if time.tv_nsec==libc::UTIME_NOW{valid|=now;}else{data[sec..sec+8].copy_from_slice(&(time.tv_sec as u64).to_le_bytes());data[nsec..nsec+4].copy_from_slice(&(time.tv_nsec as u32).to_le_bytes());}}
    if let Some(fh)=fh{valid|=64;data[8..16].copy_from_slice(&fh.to_le_bytes());}data[..4].copy_from_slice(&valid.to_le_bytes());request(route,4,node,&data).map(|_|())
}
pub fn link(old:&FuseRoute,new:&FuseRoute)->Result<(),Errno>{if old.session!=new.session{return Err(crate::errno::EXDEV);}let node=lookup(old)?;let _old=LookupGuard{route:old.clone(),node,keep:false};let(parent,name)=parent(new)?;let _parent=LookupGuard{route:new.clone(),node:parent,keep:false};let mut data=Vec::new();put64(&mut data,node);data.extend(name);request(new,13,parent,&data).and_then(|reply|forget(new,u64_at(&reply,0)?,1))}
#[cfg(test)]mod tests{
    use super::*;
    #[test]fn wire_integer_reads_reject_short_daemon_replies(){assert_eq!(u64_at(&[0;7],0),Err(EIO));assert_eq!(u32_at(&[0;3],0),Err(EIO));assert_eq!(u64_at(&17u64.to_le_bytes(),0),Ok(17));}
}
pub fn fallocate(file:&Open,offset:u64,length:u64,mode:u32)->Result<(),Errno>{let mut data=Vec::new();put64(&mut data,file.fh);put64(&mut data,offset);put64(&mut data,length);put32(&mut data,mode);put32(&mut data,0);request(&file.route,43,file.node,&data).map(|_|())}
pub fn ioctl(file:&Open,command:u32,argument:u64)->Result<i32,Errno>{
    let direction=command>>30;let size=((command>>16)&0x3fff)as usize;
    let(input,output)=(if direction&1!=0{size}else{0},if direction&2!=0{size}else{0});
    let mut data=Vec::new();put64(&mut data,file.fh);put32(&mut data,0);put32(&mut data,command);put64(&mut data,argument);put32(&mut data,input as u32);put32(&mut data,output as u32);
    if input!=0{data.extend(unsafe{std::slice::from_raw_parts(argument as *const u8,input)});}
    let reply=request(&file.route,39,file.node,&data)?;if reply.len()<16{return Err(EIO);}let flags=u32_at(&reply,4)?;if flags&4!=0{return Err(crate::errno::EOPNOTSUPP);}if u32_at(&reply,8)?!=0||u32_at(&reply,12)?!=0{return Err(EIO);}
    if reply.len()-16>output{return Err(EIO);}if reply.len()>16{unsafe{std::ptr::copy_nonoverlapping(reply[16..].as_ptr(),argument as *mut u8,reply.len()-16);}}Ok(u32_at(&reply,0)?as i32)
}
pub fn mknod(route:&FuseRoute,mode:u32,device:u32)->Result<(),Errno>{let(node,name)=parent(route)?;let _parent=LookupGuard{route:route.clone(),node,keep:false};let mut data=Vec::new();put32(&mut data,mode);put32(&mut data,device);put32(&mut data,current_umask());put32(&mut data,0);data.extend(name);request(route,8,node,&data).and_then(|reply|forget(route,u64_at(&reply,0)?,1))}

pub fn tmpfile(route:FuseRoute,guest:String,flags:u32,mode:u32)->Result<i32,Errno>{
    let parent=lookup(&route)?;let _parent=LookupGuard{route:route.clone(),node:parent,keep:false};
    let mut payload=Vec::new();put32(&mut payload,flags);put32(&mut payload,mode);put32(&mut payload,current_umask());put32(&mut payload,0);
    let reply=request(&route,51,parent,&payload)?;let node=u64_at(&reply,0)?;let fh=u64_at(&reply,128)?;let open_flags=u32_at(&reply,136)?;
    let mut opened=Open{guest,route,node,fh,flags,open_flags,broker_owned:false,directory:false,offset:std::sync::Mutex::new(0)};
    let identity=super::cred::current();let fd=super::fuse::open_description(&SessionKey::from_transport(opened.route.session.clone()),node,fh,flags,false,&opened.route.relative,&opened.guest,open_flags,&super::fuse::MountPolicy{uid:opened.route.uid,gid:opened.route.gid,allow_other:opened.route.allow_other,default_permissions:opened.route.default_permissions,read_only:opened.route.read_only},identity.uid[3],identity.gid[3],super::process::getpid() as u32)?;
    opened.broker_owned=true;let file=std::sync::Arc::new(opened);if let Err(error)=super::fuse_cache::register_file(&file){unsafe{libc::close(fd);}return Err(error);}FILES.lock().unwrap().insert(fd,file);if let Some(slow)=super::fdtab::SLOW.get(fd as usize){slow.store(1,std::sync::atomic::Ordering::Relaxed);}Ok(fd)
}
pub fn flush(file:&Open)->Result<(),Errno>{if file.flags&0o10000000!=0{return Ok(());}let mut payload=Vec::new();put64(&mut payload,file.fh);put32(&mut payload,0);put32(&mut payload,0);put64(&mut payload,super::process::getpid() as u64);request(&file.route,25,file.node,&payload).map(|_|())}
