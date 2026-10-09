//! Demand verification of mapped fs-verity pages. Fault lookup is signal-safe;
//! verification and VM changes run only after leaving the host signal handler.
use aim_storage::{fsverity::{Metadata, Identity}, private_fd::PrivateFd};
use std::{io, os::fd::AsFd, sync::{Arc, Mutex, atomic::{AtomicBool, AtomicPtr, AtomicUsize, AtomicU64, Ordering}}};

const PAGE: u64 = 16384;
const SLOTS: usize = 4096;
const RETIRED: usize = usize::MAX;
#[repr(C)]
#[derive(Default)]
struct PageInfo { disposition:i32,ref_count:i32,object_id:u64,offset:u64,depth:i32,padding:i32 }
unsafe extern "C" { fn mach_vm_page_info(task:u32,address:u64,flavor:i32,info:*mut PageInfo,count:*mut u32)->i32; }
fn private_page_copied(address:u64)->io::Result<bool> {
    let mut info=PageInfo::default();let mut count=(std::mem::size_of::<PageInfo>()/4)as u32;
    if unsafe{mach_vm_page_info(crate::patch::vm::task(),address,1,&mut info,&mut count)}!=0{return Err(io::Error::from_raw_os_error(libc::EIO));}
    Ok(info.disposition&0x20!=0)
}

pub struct Source { pub data: PrivateFd, pub proof: Option<Arc<Metadata>>, pub description: Option<Arc<super::regular_file::Description>>, pub cache:Arc<MappingCache>, pub extent:Option<(u64,u64)>, pub derivative:Option<Derivative> }
#[derive(Clone)]
pub enum Derivative { Artifact(Arc<crate::xrt::VerifiedArtifact>), Rewrite(Arc<RewritePages>), Composed(Arc<ComposedPages>) }
impl Derivative {
    fn binding(&self)->&crate::xrt::DerivationBinding{match self{Self::Artifact(artifact)=>&artifact.binding,Self::Rewrite(pages)=>&pages.plan.binding,Self::Composed(pages)=>&pages.rewrite.plan.binding}}
    fn digest(&self)->&str{match self{Self::Artifact(artifact)=>&artifact.binding.expected_output_sha256,Self::Rewrite(pages)=>&pages.digest,Self::Composed(pages)=>&pages.digest}}
    fn size(&self)->u64{match self{Self::Artifact(artifact)=>artifact.bytes().len()as u64,Self::Rewrite(pages)=>pages.plan.binding.source_offset+pages.plan.binding.source_length,Self::Composed(pages)=>pages.length}}
}
pub struct RewritePages {plan:Arc<crate::xrt::SitesRewritePlan>,bias:u64,words:Vec<(u64,u32)>,hash:Option<(u64,[u8;32])>,digest:String}
impl RewritePages {
    fn sites(plan:&crate::xrt::SitesRewritePlan,bias:u64)->io::Result<Vec<(u64,u64,crate::a64::Kind,u32)>>{
        let header=crate::elf::parse_header(plan.original_bytes()).map_err(|_|io::Error::from_raw_os_error(libc::ENOEXEC))?;
        let loads=crate::elf::parse_phdrs(plan.original_bytes(),&header).map_err(|_|io::Error::from_raw_os_error(libc::ENOEXEC))?;
        plan.sites.sites.iter().map(|&(offset,kind,rt)|{
            let segment=loads.iter().find(|segment|segment.p_type==crate::elf::PT_LOAD&&offset>=segment.p_offset&&offset.checked_add(4).is_some_and(|end|end<=segment.p_offset+segment.p_filesz)).ok_or_else(||io::Error::from_raw_os_error(libc::ENOEXEC))?;
            Ok((offset,bias+segment.p_vaddr+offset-segment.p_offset,kind,rt))
        }).collect()
    }
    pub fn prepare(plan:Arc<crate::xrt::SitesRewritePlan>,bias:u64,lo:u64,hi:u64)->io::Result<(Arc<Self>,crate::patch::PatchStats)>{
        let sites=Self::sites(&plan,bias)?;let targets:Vec<_>=sites.iter().map(|&(_,address,kind,rt)|(address,kind,rt)).collect();
        let(prepared,stats)=crate::patch::prepare_rewrite_sites(&targets,lo,hi,true);
        let prepared:std::collections::BTreeMap<_,_>=prepared.into_iter().collect();
        let words=sites.iter().map(|&(offset,address,_,_)|prepared.get(&address).copied().map(|word|(offset,word)).ok_or_else(||io::Error::from_raw_os_error(libc::EIO))).collect::<io::Result<Vec<_>>>()?;
        Ok((Arc::new(Self::restore(plan,bias,words)?),stats))
    }
    fn restore(plan:Arc<crate::xrt::SitesRewritePlan>,bias:u64,words:Vec<(u64,u32)>)->io::Result<Self>{
        let sites=Self::sites(&plan,bias)?;
        if sites.len()!=words.len(){return Err(io::Error::from_raw_os_error(libc::EIO));}
        let mut patched=plan.original_bytes().to_vec();
        for(&(offset,address,kind,rt),&(word_offset,word))in sites.iter().zip(&words){
            if offset!=word_offset||!crate::patch::validate_prepared_word(address,kind,rt,word){return Err(io::Error::from_raw_os_error(libc::EIO));}
            let start=usize::try_from(offset).map_err(|_|io::Error::from_raw_os_error(libc::EIO))?;
            patched.get_mut(start..start+4).ok_or_else(||io::Error::from_raw_os_error(libc::EIO))?.copy_from_slice(&word.to_le_bytes());
        }
        let hash=plan.sites.fips.as_ref().map(|module|{let hash=module.digest_of(&patched);module.reinject(&mut patched);(plan.binding.source_offset+module.hash_offset,hash)});
        let digest=crate::xlate::sha256_hex(&patched);
        Ok(Self{plan,bias,words,hash,digest})
    }
    fn apply(&self,offset:u64,bytes:&mut[u8])->io::Result<()> {
        for &(at,word)in &self.words {
            let at=self.plan.binding.source_offset+at;
            if at>=offset&&at<offset+bytes.len()as u64{let start=(at-offset)as usize;bytes.get_mut(start..start+4).ok_or_else(||io::Error::from_raw_os_error(libc::EIO))?.copy_from_slice(&word.to_le_bytes());}
        }
        if let Some((at,hash))=self.hash {
            let lo=at.max(offset);let hi=(at+32).min(offset+bytes.len()as u64);
            if lo<hi{bytes[(lo-offset)as usize..(hi-offset)as usize].copy_from_slice(&hash[(lo-at)as usize..(hi-at)as usize]);}
        }
        Ok(())
    }
}
pub struct ComposedPages {rewrite:Arc<RewritePages>,loads:Vec<crate::elf::Phdr>,lo:u64,length:u64,protections:Vec<i32>,digest:String}
impl ComposedPages {
    pub fn new(rewrite:Arc<RewritePages>)->io::Result<Arc<Self>> {
        let bytes=rewrite.plan.original_bytes();let header=crate::elf::parse_header(bytes).map_err(|_|io::Error::from_raw_os_error(libc::ENOEXEC))?;
        let loads:Vec<_>=crate::elf::parse_phdrs(bytes,&header).map_err(|_|io::Error::from_raw_os_error(libc::ENOEXEC))?.into_iter().filter(|load|load.p_type==crate::elf::PT_LOAD).collect();
        let lo=loads.iter().map(|load|load.p_vaddr&!(PAGE-1)).min().ok_or_else(||io::Error::from_raw_os_error(libc::ENOEXEC))?;
        let hi=loads.iter().map(|load|load.p_vaddr.checked_add(load.p_memsz).and_then(|end|end.checked_add(PAGE-1)).map(|end|end&!(PAGE-1))).collect::<Option<Vec<_>>>().ok_or_else(||io::Error::from_raw_os_error(libc::ENOEXEC))?.into_iter().max().unwrap();
        let length=hi.checked_sub(lo).ok_or_else(||io::Error::from_raw_os_error(libc::ENOEXEC))?;
        let mut protections=vec![0;usize::try_from(length/PAGE).map_err(|_|io::Error::from_raw_os_error(libc::ENOMEM))?];let mut recipe=Vec::new();
        recipe.extend_from_slice(rewrite.digest.as_bytes());recipe.extend_from_slice(&rewrite.bias.to_le_bytes());
        for load in &loads {
            if load.p_filesz>load.p_memsz||load.p_offset.checked_add(load.p_filesz).is_none_or(|end|end>bytes.len()as u64){return Err(io::Error::from_raw_os_error(libc::ENOEXEC));}
            for value in [load.p_offset,load.p_vaddr,load.p_filesz,load.p_memsz,load.p_flags as u64]{recipe.extend_from_slice(&value.to_le_bytes());}
            let mut protection=0;if load.p_flags&crate::elf::PF_R!=0{protection|=libc::PROT_READ;}if load.p_flags&crate::elf::PF_W!=0{protection|=libc::PROT_WRITE;}if load.p_flags&crate::elf::PF_X!=0{protection|=libc::PROT_EXEC;}
            let mut page=load.p_vaddr&!(PAGE-1);while page<load.p_vaddr+load.p_memsz{protections[((page-lo)/PAGE)as usize]|=protection;page+=PAGE;}
        }
        Ok(Arc::new(Self{rewrite,loads,lo,length,protections,digest:crate::xlate::sha256_hex(&recipe)}))
    }
    fn page(&self,data:&PrivateFd,proof:&Metadata,offset:u64)->io::Result<Vec<u8>> {
        let start=self.lo.checked_add(offset).ok_or_else(||io::Error::from_raw_os_error(libc::EIO))?;let end=start+PAGE;let mut page=vec![0;PAGE as usize];
        for load in &self.loads {
            let lo=start.max(load.p_vaddr);let hi=end.min(load.p_vaddr+load.p_filesz);if lo>=hi{continue;}
            let file_offset=self.rewrite.plan.binding.source_offset+load.p_offset+lo-load.p_vaddr;
            let mut bytes=proof.verify_range(data,file_offset,(hi-lo)as usize).map_err(|_|io::Error::from_raw_os_error(libc::EIO))?;
            if bytes.len()!=(hi-lo)as usize{return Err(io::Error::from_raw_os_error(libc::EIO));}self.rewrite.apply(file_offset,&mut bytes)?;
            page[(lo-start)as usize..(hi-start)as usize].copy_from_slice(&bytes);
        }
        Ok(page)
    }
}
pub struct MappingCache { directory:std::path::PathBuf }
impl MappingCache {
    pub fn new(directory:&std::path::Path)->io::Result<Self>{std::fs::create_dir_all(directory)?;Ok(Self{directory:std::fs::canonicalize(directory)?})}
    pub fn runtime()->io::Result<Arc<Self>>{let runtime=crate::vfs::runtime_dir().ok_or_else(||io::Error::from_raw_os_error(libc::ENODEV))?;Ok(Arc::new(Self::new(&runtime.join("fs-verity-page-cache"))?))}
    fn path(&self,source:&Source,offset:u64,valid:usize)->io::Result<std::path::PathBuf>{
        let identity=source.identity()?;let digest=source.cache_digest()?;
        let digest:String=digest.iter().map(|byte|format!("{byte:02x}")).collect();Ok(self.directory.join(format!("{}-{}-{offset:x}-{valid:x}-{digest}",identity.name(),if source.derivative.is_some(){"derived"}else{"original"})))
    }
    fn page(&self,source:&Source,offset:u64,bytes:&[u8])->io::Result<PrivateFd>{
        use std::os::fd::{AsRawFd,FromRawFd};use std::os::unix::fs::OpenOptionsExt;use std::io::Write;
        let path=self.path(source,offset,bytes.len())?;
        let mut binding=[0u8;PAGE as usize];binding[..8].copy_from_slice(if source.derivative.is_some(){b"AIMVPG02"}else{b"AIMVPG01"});
        binding[8..44].copy_from_slice(&source.identity()?.to_bytes());binding[44..52].copy_from_slice(&offset.to_le_bytes());binding[120..128].copy_from_slice(&(bytes.len()as u64).to_le_bytes());
        let digest=source.cache_digest()?;binding[52..56].copy_from_slice(&(digest.len()as u32).to_le_bytes());binding[56..56+digest.len()].copy_from_slice(&digest);
        let lock=aim_storage::private_fd::PrivateFile::allocate(||std::fs::OpenOptions::new().read(true).write(true).create(true).mode(0o600).custom_flags(libc::O_NOFOLLOW|libc::O_CLOEXEC).open(path.with_extension("lock")))?;
        if unsafe{libc::flock(lock.as_raw_fd(),libc::LOCK_EX)}!=0{return Err(io::Error::last_os_error());}
        if !path.exists(){
            let temporary=path.with_extension(format!("{}.tmp",std::process::id()));
            let mut file=aim_storage::private_fd::PrivateFile::allocate(||std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).custom_flags(libc::O_NOFOLLOW|libc::O_CLOEXEC).open(&temporary))?;
            file.write_all(&binding)?;file.write_all(bytes)?;if bytes.len()<PAGE as usize{file.write_all(&vec![0;PAGE as usize-bytes.len()])?;}file.sync_all()?;drop(file);std::fs::rename(&temporary,&path)?;
        }
        let mut read=aim_storage::private_fd::PrivateFile::allocate(||std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW|libc::O_CLOEXEC).open(&path))?;
        let mut actual=Vec::new();std::io::Read::read_to_end(&mut read,&mut actual)?;
        if actual.len()!=PAGE as usize*2 || actual[..PAGE as usize]!=binding || actual[PAGE as usize..PAGE as usize+bytes.len()]!=*bytes || actual[PAGE as usize+bytes.len()..].iter().any(|byte|*byte!=0) {
            return Err(io::Error::from_raw_os_error(libc::EIO));
        }
        PrivateFd::allocate(|| {
            let path=std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).map_err(|_|io::Error::from_raw_os_error(libc::EINVAL))?;
            let fd=unsafe{libc::open(path.as_ptr(),libc::O_RDONLY|libc::O_CLOEXEC|libc::O_NOFOLLOW)};
            if fd<0{return Err(io::Error::last_os_error());}Ok(unsafe{std::os::fd::OwnedFd::from_raw_fd(fd)})
        })
    }
    fn evict(&self,source:&Source,offset:u64)->io::Result<()> {
        use std::os::{fd::AsRawFd,unix::fs::OpenOptionsExt};let path=self.path(source,offset,(PAGE as usize).min(source.extent.map_or(source.size(),|(_,end)|end).saturating_sub(offset)as usize))?;
        let lock=aim_storage::private_fd::PrivateFile::allocate(||std::fs::OpenOptions::new().read(true).write(true).create(true).mode(0o600).custom_flags(libc::O_NOFOLLOW|libc::O_CLOEXEC).open(path.with_extension("lock")))?;
        if unsafe{libc::flock(lock.as_raw_fd(),libc::LOCK_EX)}!=0{return Err(io::Error::last_os_error());}
        match std::fs::remove_file(path){Ok(())=>Ok(()),Err(error)if error.kind()==io::ErrorKind::NotFound=>Ok(()),Err(error)=>Err(error)}
    }
}
impl Source {
    pub fn original(data:PrivateFd,proof:Option<Arc<Metadata>>,description:Option<Arc<super::regular_file::Description>>,cache:Arc<MappingCache>,extent:Option<(u64,u64)>)->Self{Self{data,proof,description,cache,extent,derivative:None}}
    pub fn with_derivative(mut self,artifact:Arc<crate::xrt::VerifiedArtifact>)->io::Result<Self>{
        if self.identity()?!=artifact.binding.original_identity||self.proof.is_none(){return Err(io::Error::from_raw_os_error(libc::EINVAL));}
        self.derivative=Some(Derivative::Artifact(artifact));Ok(self)
    }
    pub fn with_rewrite(mut self,pages:Arc<RewritePages>)->io::Result<Self>{
        if self.identity()?!=pages.plan.binding.original_identity||self.proof.is_none(){return Err(io::Error::from_raw_os_error(libc::EINVAL));}
        self.derivative=Some(Derivative::Rewrite(pages));Ok(self)
    }
    pub fn with_composed(mut self,pages:Arc<ComposedPages>)->io::Result<Self>{
        if self.identity()?!=pages.rewrite.plan.binding.original_identity||self.proof.is_none()||self.extent!=Some((0,pages.length)){return Err(io::Error::from_raw_os_error(libc::EINVAL));}
        self.derivative=Some(Derivative::Composed(pages));Ok(self)
    }
    fn cache_digest(&self)->io::Result<Vec<u8>>{
        if let Some(artifact)=&self.derivative {
            let digest=artifact.digest();
            if digest.len()!=64{return Err(io::Error::from_raw_os_error(libc::EIO));}
            return(0..64).step_by(2).map(|at|u8::from_str_radix(&digest[at..at+2],16).map_err(|_|io::Error::from_raw_os_error(libc::EIO))).collect();
        }
        Ok(self.proof.as_ref().ok_or_else(||io::Error::from_raw_os_error(libc::ENODATA))?.descriptor().digest())
    }
    fn size(&self)->u64{self.derivative.as_ref().map_or_else(||self.proof.as_ref().map_or(0,|proof|proof.descriptor().data_size()),|artifact|artifact.size())}
    pub fn identity(&self) -> io::Result<Identity> { Identity::from_fd(self.data.as_fd()) }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access { Read, Write, Execute }
impl Access { fn protection(self) -> i32 { match self { Self::Read => libc::PROT_READ, Self::Write => libc::PROT_WRITE, Self::Execute => libc::PROT_EXEC } } }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaultResult { Unowned, Retry, Permission, Verified, Bus { address: u64 } }
struct Mapping {
    generation:u64, base: u64, length: u64, offset: u64, logical_protection: i32, shared: bool,
    source: Arc<Source>, retired: AtomicBool, failed:AtomicBool, pending:AtomicBool, pending_gate:Mutex<bool>, pending_wake:std::sync::Condvar, operation: Mutex<Vec<PageState>>,
}
#[derive(Clone, Copy)]
struct PageState { ready: bool, protection: i32 }
struct Slot { readers: AtomicUsize, mapping: AtomicPtr<Mapping> }
impl Slot { const fn new() -> Self { Self { readers: AtomicUsize::new(RETIRED), mapping: AtomicPtr::new(std::ptr::null_mut()) } } }
static REGISTRY: [Slot; SLOTS] = [const { Slot::new() }; SLOTS];
static WRITERS: Mutex<()> = Mutex::new(());
static MAPPINGS: Mutex<Vec<Registration>> = Mutex::new(Vec::new());
static LIFECYCLE: Mutex<()> = Mutex::new(());
static NEXT_GENERATION:AtomicU64=AtomicU64::new(1);
static VM_EPOCH:std::sync::RwLock<()> = std::sync::RwLock::new(());
static MEMBERSHIPS:Mutex<std::collections::BTreeSet<[u8;36]>>=Mutex::new(std::collections::BTreeSet::new());
pub fn publish_membership(identity:Identity,admission:aim_storage::verity_control::RemoteAdmission)->io::Result<()> {
    let mut memberships=MEMBERSHIPS.lock().unwrap();let identity=identity.to_bytes();
    if !tracked_identities()?.contains(&identity){return admission.abort();}
    admission.published()?;memberships.insert(identity);Ok(())
}
pub fn tracked_identities()->io::Result<std::collections::BTreeSet<[u8;36]>> {
    MAPPINGS.lock().unwrap().iter().map(|registration|unsafe{&*registration.mapping}.source.identity().map(|identity|identity.to_bytes())).collect()
}
/// Restored VM owners join with the child's real coordinator process epoch
/// before the fork owner sends its ready acknowledgement.
pub fn restore_memberships()->io::Result<()> {
    let Some(client)=crate::verity_client()else{return Ok(());};
    for identity in tracked_identities()? {
        let admission=client.admission(identity)?;
        publish_membership(Identity::from_bytes(&identity).map_err(|_|io::Error::from_raw_os_error(libc::EINVAL))?,admission)?;
    }
    Ok(())
}
pub fn reconcile_memberships()->io::Result<()> {
    let Some(client)=crate::verity_client()else{return Ok(());};
    let mut memberships=MEMBERSHIPS.lock().unwrap();let tracked=tracked_identities()?;
    let removed:Vec<_>=memberships.difference(&tracked).copied().collect();
    for identity in removed{client.unmap(identity)?;memberships.remove(&identity);}
    Ok(())
}

pub struct VmMutation(std::sync::RwLockReadGuard<'static,()>);
pub fn mutation()->VmMutation{VmMutation(VM_EPOCH.read().unwrap())}
pub struct VmExclusive(std::sync::RwLockWriteGuard<'static,()>);
pub fn exclusive_mutation()->VmExclusive{VmExclusive(VM_EPOCH.write().unwrap())}


pub fn retain(registration: Registration) { MAPPINGS.lock().unwrap().push(registration); }
pub fn wait_pending(start:u64,length:u64) {
    loop {
        let intent=(0..SLOTS).find_map(|index| {
            let slot=&REGISTRY[index];let readers=slot.readers.load(Ordering::Acquire);
            if readers==RETIRED{return None;}
            if slot.readers.compare_exchange(readers,readers+1,Ordering::Acquire,Ordering::Relaxed).is_err(){return None;}
            let mapping=slot.mapping.load(Ordering::Acquire);
            if !mapping.is_null(){let map=unsafe{&*mapping};if start<map.base+map.length&&map.base<start.saturating_add(length)&&map.pending.load(Ordering::Acquire){return Some(FaultIntent{slot:index,mapping,address:map.base,access:Access::Read,pending:true});}}
            slot.readers.fetch_sub(1,Ordering::Release);None
        });
        let Some(intent)=intent else{return;};let _=resolve(&intent);drop(intent);
    }
}
pub fn failed_range(start:u64,length:u64)->bool {
    MAPPINGS.lock().unwrap().iter().any(|registration|{let map=unsafe{&*registration.mapping};start<map.base+map.length&&map.base<start.saturating_add(length)&&map.failed.load(Ordering::Acquire)})
}
pub fn overlaps(start: u64, length: u64) -> bool {
    let end = start.saturating_add(length);
    MAPPINGS.lock().unwrap().iter().any(|registration| {
        let map = unsafe { &*registration.mapping }; start < map.base + map.length && map.base < end
    })
}
fn overlaps_unlocked(mappings:&[Registration],start:u64,length:u64)->bool {
    mappings.iter().any(|registration|{let map=unsafe{&*registration.mapping};start<map.base+map.length&&map.base<start.saturating_add(length)})
}
pub fn remap(start:u64,old_length:u64,new:u64,new_length:u64,keep_old:bool,operation:impl FnOnce()->io::Result<()>)->io::Result<bool> {
    let _lifecycle=LIFECYCLE.lock().unwrap();
    let mut mappings=MAPPINGS.lock().unwrap();
    let Some(index)=mappings.iter().position(|registration| {let map=unsafe{&*registration.mapping};map.base==start&&map.length==old_length}) else {
        return Ok(false);
    };
    if new!=start&&overlaps_unlocked(&mappings,new,new_length){return Err(io::Error::from_raw_os_error(libc::EEXIST));}
    if keep_old&&new==start{return Err(io::Error::from_raw_os_error(libc::EINVAL));}
    let map=unsafe{&*mappings[index].mapping};
    let source=map.source.clone();let offset=map.offset;let shared=map.shared;
    let pages=map.operation.lock().unwrap().clone();let protection=pages.last().map_or(libc::PROT_READ,|page|page.protection);
    let mut registration=mappings.remove(index);registration.retire();
    if let Err(error)=operation(){
        let restored=register_inner(start,old_length,offset,protection,shared,source)?;
        *unsafe{&*restored.mapping}.operation.lock().unwrap()=pages;mappings.push(restored);return Err(error);
    }
    let next=register_inner(new,new_length,offset,protection,shared,source.clone())?;
    {let mut state=unsafe{&*next.mapping}.operation.lock().unwrap();let count=state.len().min(pages.len());state[..count].copy_from_slice(&pages[..count]);}
    mappings.push(next);
    if keep_old {
        let old=register_inner(start,old_length,offset,protection,shared,source)?;
        mappings.push(old);
    }
    Ok(true)
}

struct ForkMapping { base:u64,length:u64,offset:u64,shared:bool,source:Arc<Source>,pages:Vec<PageState> }
pub struct ForkSnapshot { mappings:Vec<ForkMapping>, carriers:Vec<super::fdtab::ForkPrivateFd> }
impl ForkSnapshot {
    pub fn capture()->io::Result<Self> {
        let mappings:Vec<ForkMapping>=MAPPINGS.lock().unwrap().iter().map(|registration| {
            let map=unsafe{&*registration.mapping};ForkMapping{base:map.base,length:map.length,offset:map.offset,shared:map.shared,source:map.source.clone(),pages:map.operation.lock().unwrap().clone()}
        }).collect();
        let mut carriers=Vec::new();let mut targets=std::collections::BTreeSet::new();
        for mapping in &mappings {
            use std::os::fd::AsRawFd;
            for descriptor in [Some(mapping.source.data.as_fd()),
                mapping.source.description.as_ref().and_then(|description|description.writer.as_ref().map(|writer|writer.descriptor()))].into_iter().flatten() {
                if targets.insert(descriptor.as_raw_fd()) {carriers.push(super::fdtab::hold_fork_private(descriptor).map_err(io::Error::from_raw_os_error)?);}
            }
        }
        Ok(Self{mappings,carriers})
    }
    /// Physical carriers only; the spawn owner must never publish them as guest FDs.
    pub fn private_fds(&self)->Vec<(i32,i32)> {
        use std::os::fd::AsRawFd;
        self.carriers.iter().map(|carrier|(carrier.source().as_raw_fd(),carrier.target())).collect()
    }
    pub fn write(&self,w:&mut super::fork_state::Writer)->io::Result<()> {
        use std::os::fd::AsRawFd;
        let identities=self.mappings.iter().map(|mapping|mapping.source.identity()).collect::<io::Result<Vec<_>>>()?;
        w.seq(self.mappings.iter().zip(identities.iter()),|w,(mapping,identity)| {
            w.u64(mapping.base);w.u64(mapping.length);w.u64(mapping.offset);w.bool(mapping.shared);w.bool(mapping.source.proof.is_some());w.opt(mapping.source.extent,|w,(start,end)|{w.u64(start);w.u64(end);});
            w.opt(mapping.source.derivative.as_ref(),|w,artifact|{
                let binding=artifact.binding();w.u64(match artifact{Derivative::Artifact(_)=>0,Derivative::Rewrite(_)=>1,Derivative::Composed(_)=>2});w.u64(binding.source_offset);w.u64(binding.source_length);w.u64(binding.translator_version as u64);w.u64(binding.ctr_el0 as u64);
                for digest in [&binding.original_sha256,&binding.expected_output_sha256,&binding.expected_sites_sha256]{w.bytes(digest.as_bytes());}
                let pages=match artifact{Derivative::Rewrite(pages)=>Some(pages),Derivative::Composed(pages)=>Some(&pages.rewrite),_=>None};
                if let Some(pages)=pages{w.u64(pages.bias);w.seq(pages.words.iter(),|w,&(offset,word)|{w.u64(offset);w.u64(word as u64);});}
            });
            w.i32(mapping.source.data.as_raw_fd());
            w.bytes(&identity.to_bytes());
            w.opt(mapping.source.description.as_ref(),|w,description| {
                w.u64(description.flags);w.opt(description.writer.as_ref(),|w,writer|w.i32(writer.descriptor().as_raw_fd()));
            });
            w.seq(mapping.pages.iter(),|w,page|{w.bool(page.ready);w.i32(page.protection);});
        });
        Ok(())
    }
    pub fn restore(r:&mut super::fork_state::Reader)->io::Result<Vec<i32>> {
        use std::os::fd::{AsFd,FromRawFd};
        let records=r.seq(|r| {
            let base=r.u64();let length=r.u64();let offset=r.u64();let shared=r.bool();let enabled=r.bool();let extent=r.opt(|r|(r.u64(),r.u64()));let derived=r.opt(|r|{let kind=r.u64();let binding=(r.u64(),r.u64(),r.u64(),r.u64(),r.bytes(),r.bytes(),r.bytes());let words=if kind>0{Some((r.u64(),r.seq(|r|(r.u64(),r.u64()))))}else{None};(kind,binding,words)});let fd=r.i32();let identity=r.bytes();
            let description=r.opt(|r|(r.u64(),r.opt(|r|r.i32())));
            let pages=r.seq(|r|PageState{ready:r.bool(),protection:r.i32()});
            (base,length,offset,shared,enabled,extent,derived,fd,identity,description,pages)
        });
        let mut consumed=std::collections::BTreeSet::new();
        for(base,length,offset,shared,enabled,extent,derived,fd,identity,description,pages)in records {
            let Some((flags,writer))=description else{return Err(io::Error::from_raw_os_error(libc::EINVAL));};
            let data=PrivateFd::allocate(|| {let copy=unsafe{libc::fcntl(fd,libc::F_DUPFD_CLOEXEC,0)};if copy<0{return Err(io::Error::last_os_error());}Ok(unsafe{std::os::fd::OwnedFd::from_raw_fd(copy)})})?;
            if Identity::from_fd(data.as_fd())?.to_bytes().to_vec() != identity {return Err(io::Error::from_raw_os_error(libc::EIO));}
            let description=super::regular_file::restore(fd,flags,writer).map_err(io::Error::from_raw_os_error)?;
            consumed.insert(fd);if let Some(writer)=writer{consumed.insert(writer);}
            let proof=description.store.lookup(description.identity).map_err(|_|io::Error::from_raw_os_error(libc::EIO))?;
            if enabled!=proof.is_some(){return Err(io::Error::from_raw_os_error(libc::EIO));}
            if pages.len()!=(length/PAGE)as usize{return Err(io::Error::from_raw_os_error(libc::EINVAL));}
            let protection=pages.last().map_or(libc::PROT_READ,|page|page.protection);
            let mut source=Source::original(data,proof.map(Arc::new),Some(description.clone()),MappingCache::runtime()?,extent);
            if let Some((kind,(source_offset,source_length,version,ctr,original_sha,output_sha,sites_sha),words))=derived {
                let original=Arc::new(super::verified_source::VerifiedSource::from_descriptor(source.data.as_fd(),Some(description)).map_err(io::Error::from_raw_os_error)?);
                let expected=crate::xrt::DerivationBinding{original_identity:source.identity()?,source_offset,source_length,translator_version:u32::try_from(version).map_err(|_|io::Error::from_raw_os_error(libc::EIO))?,ctr_el0:u32::try_from(ctr).map_err(|_|io::Error::from_raw_os_error(libc::EIO))?,original_sha256:String::from_utf8(original_sha).map_err(|_|io::Error::from_raw_os_error(libc::EIO))?,expected_output_sha256:String::from_utf8(output_sha).map_err(|_|io::Error::from_raw_os_error(libc::EIO))?,expected_sites_sha256:String::from_utf8(sites_sha).map_err(|_|io::Error::from_raw_os_error(libc::EIO))?};
                let plan=if kind==0{crate::xrt::prepare_verified_exec(original,std::path::Path::new(""),source_offset,source_length)}else{crate::xrt::verified::prepare_verified_rewrite(original,std::path::Path::new(""),&(source_offset..source_offset+source_length)).map(crate::xrt::VerifiedExecPlan::Rewrite)}.map_err(io::Error::from_raw_os_error)?;
                source=match(plan,words){
                    (crate::xrt::VerifiedExecPlan::Translated(artifact),None)if kind==0&&artifact.binding==expected=>source.with_derivative(artifact)?,
                    (crate::xrt::VerifiedExecPlan::Rewrite(plan),Some((bias,words)))if (kind==1||kind==2)&&plan.binding==expected=>{
                        let words=words.into_iter().map(|(offset,word)|u32::try_from(word).map(|word|(offset,word)).map_err(|_|io::Error::from_raw_os_error(libc::EIO))).collect::<io::Result<Vec<_>>>()?;
                        {let pages=Arc::new(RewritePages::restore(plan,bias,words)?);if kind==2{source.with_composed(ComposedPages::new(pages)?)?}else{source.with_rewrite(pages)?}}
                    },_=>return Err(io::Error::from_raw_os_error(libc::EIO)),
                };
            }
            let mapping=register(base,length,offset,protection,shared,Arc::new(source))?;
            *unsafe{&*mapping.mapping}.operation.lock().unwrap()=pages;retain(mapping);
        }
        Ok(consumed.into_iter().collect())
    }
}
/// Serialize a fixed VM replacement with owned fault retirement. The caller
/// must leave the old VM mapping intact when its operation returns an error.
pub fn replace_range(start:u64,length:u64,operation:impl FnOnce()->i64)->i64 {
    let _lifecycle=LIFECYCLE.lock().unwrap();
    let end=match start.checked_add(length){Some(end)=>end,None=>return -(libc::EINVAL as i64)};
    let mut mappings=MAPPINGS.lock().unwrap();
    let affected=mappings.iter().filter(|registration|{let map=unsafe{&*registration.mapping};start<map.base+map.length&&map.base<end}).count();
    let available=REGISTRY.iter().filter(|slot|slot.mapping.load(Ordering::Acquire).is_null()).count();
    if available<affected{return -(libc::ENOMEM as i64);}
    let mut saved=Vec::new();let mut index=0;
    while index<mappings.len() {
        let map=unsafe{&*mappings[index].mapping};
        if start>=map.base+map.length||end<=map.base{index+=1;continue;}
        saved.push(ForkMapping{base:map.base,length:map.length,offset:map.offset,shared:map.shared,source:map.source.clone(),pages:map.operation.lock().unwrap().clone()});
        let mut registration=mappings.remove(index);registration.retire();
    }
    let result=operation();
    for mapping in saved {
        let pieces:Vec<_>=if result<0 {vec![(mapping.base,mapping.base+mapping.length)]}
            else {[(mapping.base,start.min(mapping.base+mapping.length)),(end.max(mapping.base),mapping.base+mapping.length)].into_iter().filter(|(lo,hi)|lo<hi).collect()};
        for(lo,hi)in pieces {
            let first=((lo-mapping.base)/PAGE)as usize;let count=((hi-lo)/PAGE)as usize;
            let protection=mapping.pages[first].protection;
            let registration=match register_inner(lo,hi-lo,mapping.offset+lo-mapping.base,protection,mapping.shared,mapping.source.clone()) {
                Ok(registration)=>registration,Err(error)=>return -(error.raw_os_error().unwrap_or(libc::EIO)as i64),
            };
            *unsafe{&*registration.mapping}.operation.lock().unwrap()=mapping.pages[first..first+count].to_vec();mappings.push(registration);
        }
    }
    result
}
struct PreparedMapping { old:ForkMapping, backup:u64, copied:Vec<bool> }
pub struct PreparedMapsGuard { records:Vec<PreparedMapping>, proof:Arc<Metadata>, committed:bool, _epoch:std::sync::RwLockWriteGuard<'static,()> }
impl PreparedMapsGuard {
    pub fn commit(mut self)->io::Result<()> {
        if let Err(failure)=self.commit_inner(){
            if let Err(closed)=self.fail_closed(){return Err(io::Error::other(format!("mapping commit {failure}; quarantine {closed}")));}
            return Err(failure);
        }
        Ok(())
    }
    fn commit_inner(&mut self)->io::Result<()> {
        let next=self.records.iter().map(|record| {
            let data=record.old.source.data.try_clone()?;
            let source=Arc::new(Source{data,proof:Some(self.proof.clone()),description:record.old.source.description.clone(),cache:record.old.source.cache.clone(),extent:record.old.source.extent,derivative:record.old.source.derivative.clone()});
            let pages=record.old.pages.iter().zip(&record.copied).map(|(old,copied)|PageState{ready:*copied,protection:old.protection}).collect();
            Ok(Box::new(Mapping{generation:NEXT_GENERATION.fetch_add(1,Ordering::Relaxed),base:record.old.base,length:record.old.length,offset:record.old.offset,logical_protection:record.old.pages.last().map_or(libc::PROT_READ,|page|page.protection),shared:record.old.shared,source,retired:AtomicBool::new(false),failed:AtomicBool::new(false),pending:AtomicBool::new(false),pending_gate:Mutex::new(false),pending_wake:std::sync::Condvar::new(),operation:Mutex::new(pages)}))
        }).collect::<io::Result<Vec<_>>>()?;
        {
            let _lifecycle=LIFECYCLE.lock().unwrap();let mut mappings=MAPPINGS.lock().unwrap();
            let indices=self.records.iter().map(|record|mappings.iter().position(|registration|unsafe{&*registration.mapping}.base==record.old.base).ok_or_else(||io::Error::from_raw_os_error(libc::EIO))).collect::<io::Result<Vec<_>>>()?;
            for(index,next)in indices.into_iter().zip(next) {
                let registration=&mut mappings[index];let next=Box::into_raw(next);let old=registration.mapping;
                REGISTRY[registration.slot].mapping.store(next,Ordering::Release);registration.mapping=next;
                let pending=unsafe{&*old};*pending.pending_gate.lock().unwrap()=false;pending.pending.store(false,Ordering::Release);pending.pending_wake.notify_all();
                while REGISTRY[registration.slot].readers.load(Ordering::Acquire)!=0{std::thread::yield_now();}
                unsafe{drop(Box::from_raw(old));}
            }
        }
        self.committed=true;Ok(())
    }
    pub fn abort(mut self)->io::Result<()> {self.restore()?;self.committed=true;Ok(())}
    /// A published proof cannot roll back to unverified access. Keep the actual
    /// private COW objects, deny all access, and terminate pending fault waits.
    pub fn fail_closed(mut self)->io::Result<()> {
        self.committed=true;
        let _lifecycle=LIFECYCLE.lock().unwrap();let mappings=MAPPINGS.lock().unwrap();let mut failure=None;
        for record in &self.records {
            let Some(registration)=mappings.iter().find(|registration|{let map=unsafe{&*registration.mapping};map.base==record.old.base&&map.length==record.old.length})else{failure=Some(io::Error::from_raw_os_error(libc::EIO));continue;};
            let map=unsafe{&*registration.mapping};
            if unsafe{libc::mprotect(map.base as*mut _,map.length as usize,libc::PROT_NONE)}!=0{failure=Some(io::Error::last_os_error());}
            map.failed.store(true,Ordering::Release);
            *map.pending_gate.lock().unwrap()=false;map.pending.store(false,Ordering::Release);map.pending_wake.notify_all();
        }
        if let Some(error)=failure{Err(error)}else{Ok(())}
    }

    fn restore(&mut self)->io::Result<()> {
        let mappings=MAPPINGS.lock().unwrap();
        for record in &self.records {
            super::mem::remap_shared(record.old.base,record.backup,record.old.length).map_err(|error|io::Error::from_raw_os_error((-error)as i32))?;
            if let Some(registration)=mappings.iter().find(|registration|unsafe{&*registration.mapping}.base==record.old.base) {
                let map=unsafe{&*registration.mapping};*map.operation.lock().unwrap()=record.old.pages.clone();
                *map.pending_gate.lock().unwrap()=false;map.pending.store(false,Ordering::Release);map.pending_wake.notify_all();
            }
        }
        Ok(())
    }

}
impl Drop for PreparedMapsGuard {
    fn drop(&mut self){if !self.committed{if let Err(error)=self.restore(){eprintln!("verity prepare mapping rollback failed: {error}");}}
        for record in &self.records{unsafe{libc::munmap(record.backup as*mut _,record.old.length as usize);}}
    }
}
pub fn prepare_enable(identity:Identity,proof:Arc<Metadata>)->io::Result<PreparedMapsGuard> {
    let epoch=VM_EPOCH.write().unwrap();
    let _lifecycle=LIFECYCLE.lock().unwrap();let mut mappings=MAPPINGS.lock().unwrap();
    let mut guard=PreparedMapsGuard{records:Vec::new(),proof,committed:true,_epoch:epoch};
    for registration in mappings.iter() {
        let map=unsafe{&*registration.mapping};if map.source.identity()?!=identity||map.source.proof.is_some(){continue;}
        let pages=map.operation.lock().unwrap().clone();
        if map.shared&&pages.iter().any(|page|page.protection&libc::PROT_WRITE!=0){return Err(io::Error::from_raw_os_error(libc::ETXTBSY));}
        let copied=vec![false;pages.len()];
        let backup=unsafe{libc::mmap(std::ptr::null_mut(),map.length as usize,libc::PROT_NONE,libc::MAP_PRIVATE|libc::MAP_ANON,-1,0)};
        if backup==libc::MAP_FAILED{return Err(io::Error::last_os_error());}
        if let Err(error)=super::mem::remap_shared(backup as u64,map.base,map.length){unsafe{libc::munmap(backup,map.length as usize);}return Err(io::Error::from_raw_os_error((-error)as i32));}
        guard.records.push(PreparedMapping{old:ForkMapping{base:map.base,length:map.length,offset:map.offset,shared:map.shared,source:map.source.clone(),pages},backup:backup as u64,copied});
    }
    guard.committed=false;
    for registration in mappings.iter() {let map=unsafe{&*registration.mapping};if map.source.identity()?==identity&&map.source.proof.is_none(){*map.pending_gate.lock().unwrap()=true;map.pending.store(true,Ordering::Release);}}
    for record in &mut guard.records {
        // Freeze actual guest stores before querying COW provenance. VM epoch
        // alone serializes syscalls; arbitrary mapped writes need protections.
        if unsafe{libc::mprotect(record.old.base as*mut _,record.old.length as usize,libc::PROT_NONE)}!=0 {
            let error=io::Error::last_os_error();drop(mappings);drop(_lifecycle);return Err(error);
        }
        for index in 0..record.copied.len() {
            record.copied[index]=if record.old.shared{false}else{match private_page_copied(record.old.base+index as u64*PAGE){Ok(value)=>value,Err(error)=>{drop(mappings);drop(_lifecycle);return Err(error);}}};
            if record.copied[index]&&unsafe{libc::mprotect((record.old.base+index as u64*PAGE)as*mut _,PAGE as usize,record.old.pages[index].protection)}!=0 {
                let error=io::Error::last_os_error();drop(mappings);drop(_lifecycle);return Err(error);
            }
        }
    }
    Ok(guard)
}
pub struct MappingView { base:u64,length:u64,offset:u64,shared:bool,source:Arc<Source>,pages:Vec<PageState> }
pub fn mapping_view(start:u64,length:u64)->Option<MappingView>{
    let mappings=MAPPINGS.lock().unwrap();mappings.iter().find_map(|registration|{
        let map=unsafe{&*registration.mapping};if map.base!=start||map.length!=length{return None;}
        Some(MappingView{base:map.base,length:map.length,offset:map.offset,shared:map.shared,source:map.source.clone(),pages:map.operation.lock().unwrap().clone()})
    })
}
pub fn moved_existing(view:MappingView,new:u64,length:u64,keep_old:bool)->io::Result<()> {
    if !keep_old{forget(view.base,view.length)?;}
    forget(new,length)?;let protection=view.pages.last().map_or(libc::PROT_READ,|page|page.protection);
    let registration=register(new,length,view.offset,protection,view.shared,view.source)?;
    {let mut pages=unsafe{&*registration.mapping}.operation.lock().unwrap();let count=pages.len().min(view.pages.len());pages[..count].copy_from_slice(&view.pages[..count]);for page in &mut pages[count..]{page.ready=true;}}
    retain(registration);Ok(())
}
pub fn clear() -> io::Result<()> { wait_pending(0,u64::MAX);forget(0,u64::MAX)?;reconcile_memberships() }
pub fn covered(start: u64, length: u64) -> Vec<(u64,u64)> {
    let end=start.saturating_add(length); let mappings=MAPPINGS.lock().unwrap();
    let mut ranges:Vec<_>=mappings.iter().filter_map(|registration| {
        let map=unsafe{&*registration.mapping};let lo=start.max(map.base);let hi=end.min(map.base+map.length);
        (lo<hi&&map.source.proof.is_some()).then_some((lo,hi))
    }).collect();ranges.sort_unstable();ranges
}



/// Remove affected lookup slots before the VM owner changes their ranges.
/// Surviving pieces retain their original source and verified private content.
pub fn forget(start: u64, length: u64) -> io::Result<()> {
    let _lifecycle = LIFECYCLE.lock().unwrap();
    let end = start.checked_add(length).ok_or_else(|| io::Error::from_raw_os_error(libc::EINVAL))?;
    let mut mappings = MAPPINGS.lock().unwrap();
    let mut index = 0;
    while index < mappings.len() {
        let map = unsafe { &*mappings[index].mapping };
        if start >= map.base + map.length || end <= map.base { index += 1; continue; }
        let base = map.base; let len = map.length; let offset = map.offset;
        let protection = map.logical_protection; let shared = map.shared; let source = map.source.clone();
        let ready = map.operation.lock().unwrap().clone();
        // Splits replace one slot with at most two. Reserve capacity before
        // retirement so registry exhaustion cannot lose a surviving mapping.
        let pieces: Vec<_> = [(base, start.min(base + len)), (end.max(base), base + len)].into_iter().filter(|(lo,hi)|lo<hi).collect();
        if pieces.len() == 2 && !REGISTRY.iter().any(|slot| slot.mapping.load(Ordering::Acquire).is_null()) {
            return Err(io::Error::from_raw_os_error(libc::ENOMEM));
        }
        let mut registration = mappings.remove(index); registration.retire();
        for (lo, hi) in pieces {
            let piece = register_inner(lo, hi - lo, offset + lo - base, protection, shared, source.clone())?;
            let target = unsafe { &*piece.mapping }; let first = ((lo - base) / PAGE) as usize;
            *target.operation.lock().unwrap() = ready[first..first + ((hi - lo) / PAGE) as usize].to_vec();
            mappings.insert(index, piece); index += 1;
        }
    }
    Ok(())
}

pub fn note_protection(start:u64,length:u64,protection:i32){
    let mappings=MAPPINGS.lock().unwrap();for registration in mappings.iter(){let map=unsafe{&*registration.mapping};if map.source.proof.is_some(){continue;}let lo=start.max(map.base);let hi=start.saturating_add(length).min(map.base+map.length);if lo>=hi{continue;}let mut pages=map.operation.lock().unwrap();for at in(lo..hi).step_by(PAGE as usize){pages[((at-map.base)/PAGE)as usize].protection=protection;}}
}
pub fn validate_protection(start:u64,length:u64,protection:i32)->io::Result<()> {
    let end=start.checked_add(length).ok_or_else(||io::Error::from_raw_os_error(libc::EINVAL))?;
    let mappings=MAPPINGS.lock().unwrap();
    if mappings.iter().any(|registration| { let map=unsafe{&*registration.mapping}; start<map.base+map.length&&map.base<end&&map.source.proof.is_some()&&map.shared&&protection&libc::PROT_WRITE!=0 }) {
        return Err(io::Error::from_raw_os_error(libc::EACCES));
    }
    Ok(())
}
pub fn protect_range(start: u64, length: u64, protection: i32) -> io::Result<bool> {
    let _lifecycle = LIFECYCLE.lock().unwrap();
    let end = start.checked_add(length).ok_or_else(|| io::Error::from_raw_os_error(libc::EINVAL))?;
    let mappings = MAPPINGS.lock().unwrap();
    for registration in mappings.iter() {
        let map = unsafe { &*registration.mapping };
        if start < map.base + map.length && map.base < end && map.source.proof.is_some() && map.shared && protection & libc::PROT_WRITE != 0 {
            return Err(io::Error::from_raw_os_error(libc::EACCES));
        }
    }
    let mut changed: Vec<(*mut Mapping, usize, u64, PageState)> = Vec::new();
    for registration in mappings.iter() {
        let map = unsafe { &*registration.mapping }; let lo = start.max(map.base); let hi = end.min(map.base + map.length);
        if lo >= hi { continue; }
        let mut pages = map.operation.lock().unwrap();
        for at in (lo..hi).step_by(PAGE as usize) {
            let index = ((at-map.base)/PAGE) as usize; let old=pages[index];
            let result=super::mem::reprotect_verified_page(at,PAGE,if old.ready{protection}else{libc::PROT_NONE});
            if let Err(error)=result {
                drop(pages);
                for (ptr,index,address,previous) in changed.into_iter().rev() {
                    let owner: &Mapping=unsafe{&*ptr}; let mut state=owner.operation.lock().unwrap();
                    super::mem::reprotect_verified_page(address,PAGE,if previous.ready{previous.protection}else{libc::PROT_NONE})?;
                    state[index]=previous;
                }
                return Err(error);
            }
            pages[index].protection=protection;
            changed.push((registration.mapping,index,at,old));
        }
    }
    Ok(!changed.is_empty())
}

pub fn discard_range(start: u64, length: u64) -> io::Result<bool> {
    let mappings = MAPPINGS.lock().unwrap(); let end = start.checked_add(length).ok_or_else(|| io::Error::from_raw_os_error(libc::EINVAL))?;
    let mut found = false;
    for registration in mappings.iter() {
        let map = unsafe { &*registration.mapping }; let lo = start.max(map.base); let hi = end.min(map.base + map.length);
        if lo < hi { registration.discard(lo, hi - lo)?; found = true; }
    }
    Ok(found)
}

/// A retained slot reader, captured without allocating or taking a mutex.
/// The central signal trampoline must release this even when guest delivery exits.
pub struct FaultIntent { slot: usize, mapping: *mut Mapping, address: u64, access: Access, pending:bool }
unsafe impl Send for FaultIntent {}
impl Drop for FaultIntent { fn drop(&mut self) { REGISTRY[self.slot].readers.fetch_sub(1, Ordering::Release); } }
pub struct JitFault {address:u64,access:Access,generation:u64,identity:Identity}
impl FaultIntent {
    pub fn address(&self)->u64{self.address}
    pub fn jit_request(&self)->io::Result<Option<JitFault>>{
        let map=unsafe{&*self.mapping};if self.pending||map.failed.load(Ordering::Acquire)||map.shared||map.source.proof.is_none(){return Ok(None);}
        let index=((self.address-map.base)/PAGE)as usize;let protection=map.operation.lock().unwrap()[index].protection;
        if protection&(libc::PROT_READ|libc::PROT_WRITE|libc::PROT_EXEC)!=(libc::PROT_READ|libc::PROT_WRITE|libc::PROT_EXEC){return Ok(None);}
        Ok(Some(JitFault{address:self.address,access:self.access,generation:map.generation,identity:map.source.identity()?}))
    }
}
pub fn resolve_jit(request:JitFault)->FaultResult {
    let _epoch=VM_EPOCH.write().unwrap();let mappings=MAPPINGS.lock().unwrap();
    let Some(map)=mappings.iter().map(|registration|unsafe{&*registration.mapping}).find(|map|map.generation==request.generation)else{return FaultResult::Retry;};
    match map.source.identity(){Ok(identity)if identity==request.identity=>{},_=>return FaultResult::Bus{address:request.address}}
    resolve_page(map,request.address,request.access,true)
}

pub struct Registration { slot: usize, mapping: *mut Mapping }
unsafe impl Send for Registration {}
impl Registration {
    /// Discard verified residency after a mapping-owned invalidation. The next
    /// access must authenticate backing bytes again, including private COW.
    pub fn discard(&self, start: u64, length: u64) -> io::Result<()> {
        if start % PAGE != 0 || length % PAGE != 0 { return Err(io::Error::from_raw_os_error(libc::EINVAL)); }
        let map = unsafe { &*self.mapping };
        if start < map.base || start.checked_add(length).is_none_or(|end| end > map.base + map.length) {
            return Err(io::Error::from_raw_os_error(libc::EINVAL));
        }
        let mut ready = map.operation.lock().unwrap();
        if map.retired.load(Ordering::Acquire) { return Err(io::Error::from_raw_os_error(libc::EINVAL)); }
        // Replace only caller-owned pages with inaccessible empty reservations.
        let fresh = unsafe { libc::mmap(start as *mut _, length as usize, libc::PROT_NONE,
            libc::MAP_PRIVATE | libc::MAP_ANON | libc::MAP_FIXED, -1, 0) };
        if fresh == libc::MAP_FAILED { return Err(io::Error::last_os_error()); }
        for at in (start..start + length).step_by(PAGE as usize) { map.source.cache.evict(&map.source,map.offset+at-map.base)?;ready[((at - map.base) / PAGE) as usize].ready = false; }
        Ok(())
    }
    /// Retire lookup first; outstanding fault intents retain the mapping until
    /// their safe trampoline completes. VM unmap must follow this barrier.
    pub fn retire(&mut self) {
        if self.mapping.is_null() { return; }
        let _writer = WRITERS.lock().unwrap();
        let slot = &REGISTRY[self.slot];
        unsafe { (*self.mapping).retired.store(true, Ordering::Release); }
        loop {
            if slot.readers.compare_exchange(0, RETIRED, Ordering::AcqRel, Ordering::Acquire).is_ok() { break; }
            std::thread::yield_now();
        }
        slot.mapping.store(std::ptr::null_mut(), Ordering::Release);
        unsafe { drop(Box::from_raw(self.mapping)); }
        self.mapping = std::ptr::null_mut();
    }
}
impl Drop for Registration { fn drop(&mut self) { self.retire(); } }

/// The caller owns a PROT_NONE reservation. A successful registration does not
/// publish or enable fs-verity; cross-process ENABLE coordination is separate.
pub fn register_segment(base:u64,length:u64,offset:u64,file_bytes:u64,protection:i32,source:Arc<Source>)->io::Result<()> {
    if file_bytes>length{return Err(io::Error::from_raw_os_error(libc::EINVAL));}
    // Source extent carries the authenticated segment's file boundary.
    if source.extent!=Some((offset,offset+file_bytes)){return Err(io::Error::from_raw_os_error(libc::EINVAL));}
    let registration=register(base,length,offset,protection,false,source)?;
    retain(registration);Ok(())
}
pub fn register_composed(base:u64,source:Source,pages:Arc<ComposedPages>)->io::Result<()> {
    let length=pages.length;let source=Arc::new(source.with_composed(pages.clone())?);
    let registration=register(base,length,0,libc::PROT_NONE,false,source)?;
    for(state,&protection)in unsafe{&*registration.mapping}.operation.lock().unwrap().iter_mut().zip(&pages.protections){state.protection=protection;}
    retain(registration);Ok(())
}
pub fn track_existing(base:u64,length:u64,offset:u64,protection:i32,shared:bool,source:Arc<Source>)->io::Result<()> {
    let registration=register(base,length,offset,protection,shared,source)?;
    {let map=unsafe{&*registration.mapping};for page in map.operation.lock().unwrap().iter_mut(){page.ready=true;}}
    retain(registration);Ok(())
}
pub fn register(base: u64, length: u64, offset: u64, protection: i32, shared: bool, source: Arc<Source>) -> io::Result<Registration> {
    let _lifecycle = LIFECYCLE.lock().unwrap();
    register_inner(base, length, offset, protection, shared, source)
}
fn register_inner(base: u64, length: u64, offset: u64, protection: i32, shared: bool, source: Arc<Source>) -> io::Result<Registration> {
    if base % PAGE != 0 || length == 0 || length % PAGE != 0 || offset % PAGE != 0
        || base.checked_add(length).is_none() || offset.checked_add(length).is_none() {
        return Err(io::Error::from_raw_os_error(libc::EINVAL));
    }
    if source.proof.is_some() && shared && protection & libc::PROT_WRITE != 0 { return Err(io::Error::from_raw_os_error(libc::EACCES)); }
    let _writer = WRITERS.lock().unwrap();
    for slot in &REGISTRY {
        let ptr = slot.mapping.load(Ordering::Acquire);
        if !ptr.is_null() { let map = unsafe { &*ptr }; if base < map.base + map.length && map.base < base + length { return Err(io::Error::from_raw_os_error(libc::EEXIST)); } }
    }
    let slot = REGISTRY.iter().position(|slot| slot.mapping.load(Ordering::Acquire).is_null()).ok_or_else(|| io::Error::from_raw_os_error(libc::ENOMEM))?;
    let mapping = Box::into_raw(Box::new(Mapping { generation:NEXT_GENERATION.fetch_add(1,Ordering::Relaxed),base, length, offset, logical_protection: protection, shared, source, retired: AtomicBool::new(false), failed:AtomicBool::new(false), pending:AtomicBool::new(false),pending_gate:Mutex::new(false),pending_wake:std::sync::Condvar::new(), operation: Mutex::new(vec![PageState { ready: false, protection }; (length / PAGE) as usize]) }));
    REGISTRY[slot].mapping.store(mapping, Ordering::Release);
    REGISTRY[slot].readers.store(0, Ordering::Release);
    Ok(Registration { slot, mapping })
}

/// No allocation, filesystem call, mutex, or Arc clone. Bounded registry scan.
pub fn fault_intent(address: u64, access: Access) -> Option<FaultIntent> {
    for (index, slot) in REGISTRY.iter().enumerate() {
        let mut readers = slot.readers.load(Ordering::Acquire);
        loop {
            if readers == RETIRED || readers == RETIRED - 1 { break; }
            match slot.readers.compare_exchange_weak(readers, readers + 1, Ordering::Acquire, Ordering::Relaxed) {
                Ok(_) => {
                    let mapping = slot.mapping.load(Ordering::Acquire);
                    if !mapping.is_null() {
                        let map = unsafe { &*mapping };
                        if address >= map.base && address < map.base + map.length && !map.retired.load(Ordering::Acquire) && (map.source.proof.is_some() || map.pending.load(Ordering::Acquire)||map.failed.load(Ordering::Acquire)) {
                            return Some(FaultIntent { slot: index, mapping, address, access, pending:map.pending.load(Ordering::Acquire) });
                        }
                    }
                    slot.readers.fetch_sub(1, Ordering::Release);
                    break;
                }
                Err(current) => readers = current,
            }
        }
    }
    None
}

/// Runs on a safe host stack, never inside SA_SIGINFO. All bytes in the host
/// page are authenticated before any part is exposed to the faulting access.
pub fn resolve(intent: &FaultIntent) -> FaultResult {
    let map = unsafe { &*intent.mapping };
    if map.failed.load(Ordering::Acquire){return FaultResult::Bus{address:intent.address};}
    if intent.pending {
        let mut pending=map.pending_gate.lock().unwrap();
        while *pending {pending=map.pending_wake.wait(pending).unwrap();}
        return if map.failed.load(Ordering::Acquire){FaultResult::Bus{address:intent.address}}else{FaultResult::Retry};
    }
    resolve_page(map,intent.address,intent.access,false)
}
/// Resolve lazy user pages before a kernel copy, outside descriptor/VM locks.
pub fn prepare_user_range(address:u64,length:u64,access:Access)->Result<(),crate::errno::Errno>{
    if length==0{return Ok(());}
    if address==0{return Err(crate::errno::EFAULT);}
    let end=address.checked_add(length).ok_or(crate::errno::EFAULT)?;
    let mut at=address;
    while at<end {
        loop {
            let Some(intent)=fault_intent(at,access)else{break;};
            let request=intent.jit_request().map_err(|error|crate::errno::from_darwin(error.raw_os_error().unwrap_or(libc::EIO)))?;
            let result=if let Some(request)=request{drop(intent);resolve_jit(request)}else{let result=resolve(&intent);drop(intent);result};
            match result {
                FaultResult::Verified|FaultResult::Unowned=>break,
                FaultResult::Retry=>continue,
                FaultResult::Permission=>return Err(crate::errno::EFAULT),
                FaultResult::Bus{..}=>return Err(crate::errno::EIO),
            }
        }
        at=(at&!(PAGE-1)).checked_add(PAGE).ok_or(crate::errno::EFAULT)?.min(end);
    }
    Ok(())
}
fn resolve_page(map:&Mapping,address:u64,access:Access,jit:bool)->FaultResult {
    if map.failed.load(Ordering::Acquire){return FaultResult::Bus{address};}
    if map.source.proof.is_none(){return FaultResult::Unowned;}
    let mut ready = map.operation.lock().unwrap();
    if map.retired.load(Ordering::Acquire) { return FaultResult::Unowned; }
    let page = address & !(PAGE - 1);
    let index = ((page - map.base) / PAGE) as usize;
    let protection = ready[index].protection;
    if protection & access.protection() == 0 { return FaultResult::Permission; }
    if ready[index].ready { return FaultResult::Verified; }
    let offset = map.offset + page - map.base;
    if offset >= map.source.size() { return FaultResult::Bus { address: address }; }
    let file_length=map.source.extent.map(|(_,end)|end.saturating_sub(offset).min(PAGE)as usize).unwrap_or(PAGE as usize);
    let bytes=match &map.source.derivative {
        Some(Derivative::Artifact(artifact))=>{let length=file_length.min(artifact.bytes().len().saturating_sub(offset as usize));let mut bytes=vec![0;length];artifact.read_exact_at(&mut bytes,offset).map(|()|bytes).map_err(|_|())},
        Some(Derivative::Rewrite(pages))=>map.source.proof.as_ref().unwrap().verify_range(&map.source.data,offset,file_length).map_err(|_|()).and_then(|mut bytes|pages.apply(offset,&mut bytes).map(|()|bytes).map_err(|_|())),
        Some(Derivative::Composed(pages))=>pages.page(&map.source.data,map.source.proof.as_ref().unwrap(),offset).map_err(|_|()),
        None=>map.source.proof.as_ref().unwrap().verify_range(&map.source.data,offset,file_length).map_err(|_|()),
    };
    let verified = match bytes {
        Ok(bytes) => bytes,
        Err(_) => return FaultResult::Bus { address: address },
    };
    if jit {
        let mut bytes=vec![0;PAGE as usize];bytes[..verified.len()].copy_from_slice(&verified);
        let result=super::mem::publish_verified_jit_page(page,&bytes);
        return match result{Ok(())=>{ready[index].ready=true;FaultResult::Verified},Err(error)=>{crate::diag!("[linux-abi] verified JIT publish at {page:#x}: {error}");FaultResult::Bus{address}}};
    }
    let cache=match map.source.cache.page(&map.source,offset,&verified){Ok(cache)=>cache,Err(_)=>return FaultResult::Bus{address:address}};
    use std::os::fd::AsRawFd;
    let staging=unsafe{libc::mmap(std::ptr::null_mut(),PAGE as usize,libc::PROT_READ,if map.shared{libc::MAP_SHARED}else{libc::MAP_PRIVATE},cache.as_raw_fd(),PAGE as i64)};
    if staging==libc::MAP_FAILED{return FaultResult::Bus{address:address};}
    let result=super::mem::remap_verified_page(page,staging as u64,PAGE,protection,!map.shared);
    unsafe{libc::munmap(staging,PAGE as usize);}

    match result {Ok(())=>{ready[index].ready=true;FaultResult::Verified},Err(error)=>{crate::diag!("[linux-abi] verified page publish at {page:#x}: {error}");FaultResult::Bus{address:address}}}
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs::{self, File}, os::unix::fs::FileExt};
    #[test]
    fn published_failure_wakes_faults_without_restoring_unverified_access() {
        let root=std::env::temp_dir().join(format!("aim-verity-fail-{}",std::process::id()));fs::create_dir(&root).unwrap();fs::write(root.join("data"),vec![37;PAGE as usize*2]).unwrap();let file=File::open(root.join("data")).unwrap();
        let store=aim_storage::fsverity::Store::new(&root.join("proof"),&root.join("runtime")).unwrap();let admission=store.lock_inode(&file).unwrap();let enable=admission.begin_enable().unwrap();drop(admission);
        let prepared=enable.build(aim_storage::fsverity::BuildOptions::new(1,4096,vec![],PAGE,4096).unwrap(),&[],||false).unwrap();let proof=Arc::new(prepared.metadata_view().unwrap());
        use std::os::fd::AsRawFd;let memory=unsafe{libc::mmap(std::ptr::null_mut(),PAGE as usize*2,libc::PROT_READ|libc::PROT_WRITE,libc::MAP_PRIVATE,file.as_raw_fd(),0)};assert_ne!(memory,libc::MAP_FAILED);let base=memory as u64;unsafe{std::ptr::write_volatile(memory.cast::<u8>(),91);}
        let identity=Identity::from_fd(file.as_fd()).unwrap();track_existing(base,PAGE*2,0,libc::PROT_READ|libc::PROT_WRITE,false,Arc::new(Source::original(PrivateFd::adopt(file.into()).unwrap(),None,None,Arc::new(MappingCache::new(&root.join("cache")).unwrap()),None))).unwrap();
        let guard=prepare_enable(identity,proof).unwrap();let intent=fault_intent(base+PAGE+17,Access::Read).unwrap();let(tx,rx)=std::sync::mpsc::channel();let waiter=std::thread::spawn(move||{tx.send(resolve(&intent)).unwrap();});assert!(rx.recv_timeout(std::time::Duration::from_millis(20)).is_err());
        guard.fail_closed().unwrap();assert_eq!(rx.recv_timeout(std::time::Duration::from_secs(1)).unwrap(),FaultResult::Bus{address:base+PAGE+17});waiter.join().unwrap();
        assert_eq!(super::super::vmmap::info_at(base).unwrap().prot&7,0);let intent=fault_intent(base,Access::Write).unwrap();assert_eq!(resolve(&intent),FaultResult::Bus{address:base});drop(intent);
        assert_eq!(super::super::mem::mprotect([base,PAGE,libc::PROT_READ as u64,0,0,0]),-(crate::errno::EIO as i64));
        let alias=unsafe{libc::mmap(std::ptr::null_mut(),PAGE as usize*2,libc::PROT_NONE,libc::MAP_PRIVATE|libc::MAP_ANON,-1,0)};assert_ne!(alias,libc::MAP_FAILED);super::super::mem::remap_shared(alias as u64,base,PAGE*2).unwrap();assert_eq!(unsafe{libc::mprotect(alias,PAGE as usize*2,libc::PROT_READ)},0);assert_eq!(unsafe{std::ptr::read_volatile(alias.cast::<u8>())},91);assert_eq!(unsafe{std::ptr::read_volatile(alias.cast::<u8>().add(PAGE as usize))},37);
        {let _epoch=mutation();}forget(base,PAGE*2).unwrap();unsafe{libc::munmap(memory,PAGE as usize*2);libc::munmap(alias,PAGE as usize*2);}drop(enable);drop(prepared);fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn local_prepare_abort_restores_actual_private_cow_and_original_mapping() {
        use std::os::fd::AsRawFd;
        let root=std::env::temp_dir().join(format!("aim-verity-prepare-{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir(&root).unwrap();fs::write(root.join("data"),vec![37;PAGE as usize*2]).unwrap();let file=File::open(root.join("data")).unwrap();
        let store=aim_storage::fsverity::Store::new(&root.join("proof"),&root.join("runtime")).unwrap();let admission=store.lock_inode(&file).unwrap();let guard=admission.begin_enable().unwrap();drop(admission);
        let blob=guard.build(aim_storage::fsverity::BuildOptions::new(1,4096,vec![],PAGE,4096).unwrap(),&[],||false).unwrap();let proof=Arc::new(blob.metadata_view().unwrap());
        assert!(store.lookup(Identity::from_fd(file.as_fd()).unwrap()).unwrap().is_none());
        let memory=unsafe{libc::mmap(std::ptr::null_mut(),PAGE as usize*2,libc::PROT_READ|libc::PROT_WRITE,libc::MAP_PRIVATE,file.as_raw_fd(),0)};assert_ne!(memory,libc::MAP_FAILED);let base=memory as u64;
        unsafe{std::ptr::write_volatile(memory.cast::<u8>(),91)};
        let data=PrivateFd::adopt(file.into()).unwrap();let identity=Identity::from_fd(data.as_fd()).unwrap();
        track_existing(base,PAGE*2,0,libc::PROT_READ|libc::PROT_WRITE,false,Arc::new(Source{data,proof:None,description:None,cache:Arc::new(MappingCache::new(&root.join("cache")).unwrap()),extent:None,derivative:None})).unwrap();
        let writable=File::options().read(true).write(true).open(root.join("data")).unwrap();
        let shared=unsafe{libc::mmap(std::ptr::null_mut(),PAGE as usize,libc::PROT_READ|libc::PROT_WRITE,libc::MAP_SHARED,writable.as_raw_fd(),0)};assert_ne!(shared,libc::MAP_FAILED);
        let data=PrivateFd::adopt(writable.into()).unwrap();
        track_existing(shared as u64,PAGE,0,libc::PROT_READ|libc::PROT_WRITE,true,Arc::new(Source{data,proof:None,description:None,cache:Arc::new(MappingCache::new(&root.join("cache")).unwrap()),extent:None,derivative:None})).unwrap();
        assert_eq!(prepare_enable(identity,proof.clone()).err().unwrap().raw_os_error(),Some(libc::ETXTBSY));
        assert_eq!(unsafe{std::ptr::read_volatile(memory.cast::<u8>())},91);
        forget(shared as u64,PAGE).unwrap();unsafe{libc::munmap(shared,PAGE as usize);}
        let prepared=prepare_enable(identity,proof).unwrap();assert_eq!(unsafe{std::ptr::read_volatile(memory.cast::<u8>())},91);
        let mutator_started=Arc::new(AtomicBool::new(false));let started=mutator_started.clone();
        let mutator_finished=Arc::new(AtomicBool::new(false));let finished=mutator_finished.clone();
        let mutator=std::thread::spawn(move||{started.store(true,Ordering::Release);let _epoch=mutation();assert_eq!(unsafe{std::ptr::read_volatile((base+PAGE)as*const u8)},37);finished.store(true,Ordering::Release);});
        while !mutator_started.load(Ordering::Acquire){std::thread::yield_now();}
        assert!(!mutator_finished.load(Ordering::Acquire));
        let waiting=fault_intent(base+PAGE,Access::Read).unwrap();
        let entered=Arc::new(AtomicBool::new(false));let ready=entered.clone();
        let fault=std::thread::spawn(move||{ready.store(true,Ordering::Release);resolve(&waiting)});
        while !entered.load(Ordering::Acquire){std::thread::yield_now();}
        prepared.abort().unwrap();assert_eq!(fault.join().unwrap(),FaultResult::Retry);mutator.join().unwrap();assert!(mutator_finished.load(Ordering::Acquire));assert!(store.lookup(identity).unwrap().is_none());assert_eq!(unsafe{std::ptr::read_volatile(memory.cast::<u8>())},91);assert_eq!(unsafe{std::ptr::read_volatile((base+PAGE)as*const u8)},37);
        assert!(fault_intent(base,Access::Read).is_none());
        let view=Arc::new(blob.metadata_view().unwrap());
        let committed=prepare_enable(identity,view).unwrap();assert_eq!(committed.records[0].copied,[true,false]);committed.commit().unwrap();
        assert_eq!(unsafe{std::ptr::read_volatile(memory.cast::<u8>())},91);
        let corrupt=File::options().write(true).open(root.join("data")).unwrap();corrupt.write_all_at(&[99],PAGE+20).unwrap();drop(corrupt);
        let clean=fault_intent(base+PAGE+20,Access::Read).unwrap();assert_eq!(resolve(&clean),FaultResult::Bus{address:base+PAGE+20});drop(clean);
        assert_eq!(unsafe{std::ptr::read_volatile(memory.cast::<u8>())},91);assert!(store.lookup(identity).unwrap().is_none());
        forget(base,PAGE*2).unwrap();unsafe{libc::munmap(memory,PAGE as usize*2);}fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn private_cow_provenance_is_reported_by_actual_mach_page_owner() {
        use std::os::fd::AsRawFd;
        let root=std::env::temp_dir().join(format!("aim-verity-cow-{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        fs::write(&root,vec![37;PAGE as usize*2]).unwrap();let file=File::open(&root).unwrap();
        let map=unsafe{libc::mmap(std::ptr::null_mut(),PAGE as usize*2,libc::PROT_READ|libc::PROT_WRITE,libc::MAP_PRIVATE,file.as_raw_fd(),0)};
        assert_ne!(map,libc::MAP_FAILED);let base=map as u64;
        assert_eq!(unsafe{std::ptr::read_volatile(map.cast::<u8>())},37);
        assert!(!private_page_copied(base).unwrap());
        unsafe{std::ptr::write_volatile(map.cast::<u8>(),91)};
        assert!(private_page_copied(base).unwrap());
        assert!(!private_page_copied(base+PAGE).unwrap());
        assert_eq!(unsafe{libc::munmap(map,PAGE as usize*2)},0);drop(file);fs::remove_file(root).unwrap();
    }
    #[test]
    fn worker_verifies_touched_page_before_publish_and_rejects_corrupt_neighbor() {
        let root = std::env::temp_dir().join(format!("aim-verity-pager-{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir(&root).unwrap();
        let bytes = vec![37; PAGE as usize * 2]; fs::write(root.join("data"), &bytes).unwrap();
        let data = File::open(root.join("data")).unwrap();
        let store = aim_storage::fsverity::Store::new(&root.join("proof"), &root.join("runtime")).unwrap();
        let admission = store.lock_inode(&data).unwrap(); let guard = admission.begin_enable().unwrap(); drop(admission);
        let prepared = guard.build(aim_storage::fsverity::BuildOptions::new(1,4096,vec![],PAGE,4096).unwrap(),&[],||false).unwrap();
        let proof = Arc::new(guard.commit(prepared).unwrap());
        let source = Arc::new(Source { data: PrivateFd::adopt(data.into()).unwrap(), proof:Some(proof), description: None,cache:Arc::new(MappingCache::new(&root.join("cache")).unwrap()),extent:None,derivative:None });
        let memory = unsafe { libc::mmap(std::ptr::null_mut(),(PAGE*3) as usize,libc::PROT_NONE,libc::MAP_PRIVATE|libc::MAP_ANON,-1,0) };
        assert_ne!(memory,libc::MAP_FAILED); let base=memory as u64;
        let mut registration = register(base,PAGE*2,0,libc::PROT_READ|libc::PROT_WRITE,false,source.clone()).unwrap();
        assert!(fault_intent(base+PAGE*2,Access::Read).is_none());
        let first = fault_intent(base+17,Access::Read).unwrap();
        assert_eq!(std::thread::spawn(move||resolve(&first)).join().unwrap(),FaultResult::Verified);
        assert_eq!(unsafe{std::ptr::read_volatile((base+17)as*const u8)},37);
        unsafe{std::ptr::write_volatile((base+17)as*mut u8,91)};
        let repeated=fault_intent(base+17,Access::Write).unwrap();assert_eq!(resolve(&repeated),FaultResult::Verified);drop(repeated);
        assert_eq!(unsafe{std::ptr::read_volatile((base+17)as*const u8)},91); // COW content is not republished over a completed page.
        registration.discard(base, PAGE).unwrap();
        let reread = fault_intent(base + 17, Access::Read).unwrap();
        assert_eq!(resolve(&reread), FaultResult::Verified); drop(reread);
        assert_eq!(unsafe{std::ptr::read_volatile((base+17)as*const u8)},37);
        let corrupt=File::options().write(true).open(root.join("data")).unwrap();corrupt.write_all_at(&[99],PAGE+20).unwrap();drop(corrupt);
        let second=fault_intent(base+PAGE+20,Access::Read).unwrap();assert_eq!(resolve(&second),FaultResult::Bus{address:base+PAGE+20});drop(second);
        let execute=fault_intent(base,Access::Execute).unwrap();assert_eq!(resolve(&execute),FaultResult::Permission);drop(execute);
        assert_eq!(register(base+PAGE*2,PAGE,0,libc::PROT_WRITE,true,source.clone()).err().unwrap().raw_os_error(),Some(libc::EACCES));
        let held = fault_intent(base, Access::Read).unwrap();
        let retired = Arc::new(AtomicBool::new(false));
        let finished = retired.clone();
        let cleanup = std::thread::spawn(move || { registration.retire(); finished.store(true, Ordering::Release); });
        while fault_intent(base, Access::Read).is_some() { std::thread::yield_now(); }
        assert!(!retired.load(Ordering::Acquire));
        drop(held); cleanup.join().unwrap();
        assert!(retired.load(Ordering::Acquire)); assert!(fault_intent(base,Access::Read).is_none());
        assert_eq!(unsafe{libc::munmap(memory,(PAGE*3)as usize)},0);
        let reservation = unsafe { libc::mmap(std::ptr::null_mut(), (PAGE*3) as usize, libc::PROT_NONE, libc::MAP_PRIVATE|libc::MAP_ANON, -1, 0) };
        assert_ne!(reservation, libc::MAP_FAILED); let address=reservation as u64;
        retain(register(address,PAGE*3,0,libc::PROT_READ|libc::PROT_WRITE,false,source).unwrap());
        let good=fault_intent(address+1,Access::Read).unwrap();assert_eq!(resolve(&good),FaultResult::Verified);drop(good);
        assert!(protect_range(address,PAGE,libc::PROT_READ).unwrap());
        let denied=fault_intent(address+1,Access::Write).unwrap();assert_eq!(resolve(&denied),FaultResult::Permission);drop(denied);
        assert!(discard_range(address,PAGE).unwrap());
        let rechecked=fault_intent(address+1,Access::Read).unwrap();assert_eq!(resolve(&rechecked),FaultResult::Verified);drop(rechecked);
        assert_eq!(replace_range(address+PAGE,PAGE,||-(libc::ENOMEM as i64)),-(libc::ENOMEM as i64));
        assert!(fault_intent(address+PAGE+1,Access::Read).is_some());
        assert_eq!(replace_range(address+PAGE,PAGE,||0),0);
        assert!(fault_intent(address+PAGE+1,Access::Read).is_none());
        assert!(fault_intent(address+1,Access::Read).is_some());assert!(fault_intent(address+PAGE*2+1,Access::Read).is_some());
        forget(address,PAGE*3).unwrap();assert!(fault_intent(address+1,Access::Read).is_none());
        assert_eq!(unsafe{libc::munmap(reservation,(PAGE*3)as usize)},0);
        fs::remove_dir_all(root).unwrap();
    }
}
