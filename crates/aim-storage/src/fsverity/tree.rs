use super::{Error,Result,EIO,EINVAL,EOVERFLOW,EMSGSIZE};
use sha2::{Digest,Sha256,Sha512};
use std::os::fd::{AsFd,AsRawFd};

#[derive(Clone,Debug,PartialEq,Eq)]
pub struct BuildOptions{pub algorithm:u8,pub block_size:usize,pub salt:Vec<u8>}
impl BuildOptions {
 pub fn new(algorithm:u8,block_size:usize,salt:Vec<u8>,page_size:u64,fs_block_size:u64)->Result<Self>{
  if salt.len()>32{return Err(Error::Linux(EMSGSIZE));}
  if !matches!(algorithm,1|2)||!block_size.is_power_of_two()||block_size<1024||block_size as u64>page_size.min(fs_block_size){return Err(Error::Linux(EINVAL));}
  Ok(Self{algorithm,block_size,salt})
 }
 pub fn from_enable_arg(arg:&[u8;128],salt:Vec<u8>,page_size:u64,fs_block_size:u64)->Result<Self>{
  let word=|offset|u32::from_le_bytes(arg[offset..offset+4].try_into().unwrap());
  if word(0)!=1||word(28)!=0||arg[40..].iter().any(|byte|*byte!=0){return Err(Error::Linux(EINVAL));}
  if !word(8).is_power_of_two(){return Err(Error::Linux(EINVAL));}
  if word(12)>32||word(24)>16128{return Err(Error::Linux(EMSGSIZE));}
  if word(12)as usize!=salt.len(){return Err(Error::Linux(EINVAL));}
  Self::new(u8::try_from(word(4)).map_err(|_|Error::Linux(EINVAL))?,word(8)as usize,salt,page_size,fs_block_size)
 }
 pub fn digest_size(&self)->usize{if self.algorithm==1{32}else{64}}
 pub(crate) fn hash(&self,bytes:&[u8])->Vec<u8>{
  let mut salt=self.salt.clone();if !salt.is_empty(){salt.resize(if self.algorithm==1{64}else{128},0);}
  if self.algorithm==1{let mut hash=Sha256::new();hash.update(salt);hash.update(bytes);hash.finalize().to_vec()}
  else{let mut hash=Sha512::new();hash.update(salt);hash.update(bytes);hash.finalize().to_vec()}
 }
}
#[derive(Clone,Debug)]
pub struct Descriptor{bytes:[u8;256],options:BuildOptions}
impl Descriptor {
 pub fn from_bytes(bytes:[u8;256])->Result<Self>{
  if bytes[0]!=1||!matches!(bytes[1],1|2)||!(10..=16).contains(&bytes[2])||bytes[3]>32||bytes[4..8].iter().chain(bytes[112..].iter()).any(|byte|*byte!=0){return Err(Error::Linux(EIO));}
  let options=BuildOptions{algorithm:bytes[1],block_size:1usize<<bytes[2],salt:bytes[80..80+bytes[3] as usize].to_vec()};
  if bytes[16+options.digest_size()..80].iter().chain(bytes[80+options.salt.len()..112].iter()).any(|byte|*byte!=0){return Err(Error::Linux(EIO));}
  Ok(Self{bytes,options})
 }
 pub fn bytes(&self)->&[u8;256]{&self.bytes}
 pub fn options(&self)->&BuildOptions{&self.options}
 pub fn data_size(&self)->u64{u64::from_le_bytes(self.bytes[8..16].try_into().unwrap())}
 pub fn digest(&self)->Vec<u8>{if self.options.algorithm==1{Sha256::digest(self.bytes).to_vec()}else{Sha512::digest(self.bytes).to_vec()}}
 pub(crate) fn root(&self)->&[u8]{&self.bytes[16..16+self.options.digest_size()]}
 pub(crate) fn levels(&self)->Result<Vec<(u64,u64)>>{levels(self.data_size(),&self.options)}
}
// Leaf-to-root levels, with their offsets in the root-first Linux tree layout.
fn levels(size:u64,options:&BuildOptions)->Result<Vec<(u64,u64)>>{
 let mut hashes=size.div_ceil(options.block_size as u64);let fanout=(options.block_size/options.digest_size()) as u64;
 let mut sizes=Vec::new();while hashes>1{hashes=hashes.div_ceil(fanout);sizes.push(hashes.checked_mul(options.block_size as u64).ok_or(Error::Linux(EOVERFLOW))?);}
 let mut offset=0u64;let mut out=vec![(0,0);sizes.len()];for index in (0..sizes.len()).rev(){out[index]=(offset,sizes[index]);offset=offset.checked_add(sizes[index]).ok_or(Error::Linux(EOVERFLOW))?;}Ok(out)
}
pub(crate) fn read_exact(file:&impl AsFd,bytes:&mut[u8],offset:u64)->Result<()>{
 let mut at=0;while at<bytes.len(){let position=i64::try_from(offset.checked_add(at as u64).ok_or(Error::Linux(EOVERFLOW))?).map_err(|_|Error::Linux(EOVERFLOW))?;let n=unsafe{libc::pread(file.as_fd().as_raw_fd(),bytes[at..].as_mut_ptr().cast(),bytes.len()-at,position)};if n<0{return Err(std::io::Error::last_os_error().into());}let n=n as usize;if n==0{return Err(Error::Linux(EIO));}at+=n;}Ok(())
}
pub(crate) fn write_all(file:&impl AsFd,bytes:&[u8],offset:u64)->Result<()>{
 let mut at=0;while at<bytes.len(){let position=i64::try_from(offset.checked_add(at as u64).ok_or(Error::Linux(EOVERFLOW))?).map_err(|_|Error::Linux(EOVERFLOW))?;let n=unsafe{libc::pwrite(file.as_fd().as_raw_fd(),bytes[at..].as_ptr().cast(),bytes.len()-at,position)};if n<0{return Err(std::io::Error::last_os_error().into());}let n=n as usize;if n==0{return Err(Error::Linux(EIO));}at+=n;}Ok(())
}
pub(crate) fn stat(file:&impl AsFd)->Result<libc::stat>{
 let mut stat:libc::stat=unsafe{std::mem::zeroed()};
 if unsafe{libc::fstat(file.as_fd().as_raw_fd(),&mut stat)}<0{return Err(std::io::Error::last_os_error().into());}Ok(stat)
}
/// Build into a private metadata file using a block-sized working buffer.
/// The caller owns exclusive write admission; this function does not enable verity.
pub fn build(data:&impl AsFd,tree:&impl AsFd,tree_offset:u64,options:BuildOptions,mut interrupted:impl FnMut()->bool)->Result<Descriptor>{
 let options=BuildOptions::new(options.algorithm,options.block_size,options.salt,65536,65536)?;
 let size=stat(data)?.st_size as u64;let layout=levels(size,&options)?;
 let blocks=size.div_ceil(options.block_size as u64);let mut buffer=vec![0;options.block_size];
 let mut root=vec![0;options.digest_size()];
 if blocks==1{read_exact(data,&mut buffer[..size as usize],0)?;root=options.hash(&buffer);}
 else if blocks>1 {
  let fanout=(options.block_size/options.digest_size()) as u64;
  for index in 0..blocks{
   if interrupted(){return Err(Error::Linux(4));}buffer.fill(0);let offset=index*options.block_size as u64;let n=(size-offset).min(options.block_size as u64) as usize;
   read_exact(data,&mut buffer[..n],offset)?;write_all(tree,&options.hash(&buffer),tree_offset+layout[0].0+index*options.digest_size() as u64)?;
  }
  for level in 0..layout.len(){
   let(offset,length)=layout[level];let hashes=if level==0{blocks}else{layout[level-1].1/options.block_size as u64};
   let used=hashes*options.digest_size() as u64;let padding=(length-used) as usize;
   if padding>0{write_all(tree,&vec![0;padding],tree_offset+offset+used)?;}
   for index in 0..length/options.block_size as u64{
    if interrupted(){return Err(Error::Linux(4));}read_exact(tree,&mut buffer,tree_offset+offset+index*options.block_size as u64)?;
    let hash=options.hash(&buffer);
    if level+1==layout.len(){root=hash;}else{write_all(tree,&hash,tree_offset+layout[level+1].0+index*options.digest_size() as u64)?;}
   }
   debug_assert_eq!(length/options.block_size as u64,hashes.div_ceil(fanout));
  }
 }
 let mut bytes=[0;256];bytes[0]=1;bytes[1]=options.algorithm;bytes[2]=options.block_size.trailing_zeros() as u8;bytes[3]=options.salt.len() as u8;
 bytes[8..16].copy_from_slice(&size.to_le_bytes());bytes[16..16+root.len()].copy_from_slice(&root);bytes[80..80+options.salt.len()].copy_from_slice(&options.salt);
 if interrupted(){return Err(Error::Linux(4));}Ok(Descriptor{bytes,options})
}

#[cfg(test)]
mod tests {
 use super::*;
 use std::{fs::{self,File},path::PathBuf};
 struct Data(PathBuf);
 impl Data{fn new(bytes:&[u8])->Self{let path=std::env::temp_dir().join(format!("aim-verity-core-{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));fs::create_dir(&path).unwrap();fs::write(path.join("data"),bytes).unwrap();Self(path)}fn data(&self)->File{File::open(self.0.join("data")).unwrap()}fn tree(&self)->File{File::options().create_new(true).read(true).write(true).open(self.0.join("tree")).unwrap()}}
 impl Drop for Data{fn drop(&mut self){fs::remove_dir_all(&self.0).unwrap();}}
 fn hex(bytes:&[u8])->String{bytes.iter().map(|byte|format!("{byte:02x}")).collect()}
 #[test]
 fn linux_uapi_descriptor_vectors_cover_zero_tail_salt_and_both_hashes(){
  // Exact Linux v6.6 UAPI descriptor; independently computed hashlib vectors.
  // https://github.com/torvalds/linux/blob/v6.6/include/uapi/linux/fsverity.h
  for(bytes,algorithm,salt,expected)in[
   (b"".as_slice(),1,vec![],"3d248ca542a24fc62d1c43b916eae5016878e2533c88238480b26128a1f1af95"),
   (b"abc".as_slice(),1,vec![],"700b6bd8510f0b4f9bac8b9cf0459151a1c4a99f467892bb4bd289a67df8e19c"),
   (b"abc".as_slice(),1,vec![1,2],"66fa83c364c5dfd53d984901215c7f6f3737665351965f03881bc7b02aed139f"),
   (b"abc".as_slice(),2,vec![],"78be1be69d611f5b6b013eb333311beccea25ab099b68ecd4e6ed6bf5175966c7c5bce19fca5f218848fd0ecd3cc71246b9dc3d45ce9f05a4e808b8e28439517")]{
   let files=Data::new(bytes);let proof=build(&files.data(),&files.tree(),512,BuildOptions::new(algorithm,4096,salt,16384,4096).unwrap(),||false).unwrap();
   assert_eq!(hex(&proof.digest()),expected);assert_eq!(proof.data_size(),bytes.len() as u64);assert_eq!(Descriptor::from_bytes(*proof.bytes()).unwrap().digest(),proof.digest());
  }
 }
 #[test]
 fn upstream_fsverity_utils_digest_vectors(){
  // MIT-licensed test vectors, Google LLC, programs/test_compute_digest.c:
  // https://github.com/ebiggers/fsverity-utils/blob/master/programs/test_compute_digest.c
  for(size,algorithm,block,salt,expected)in[
   (1000000usize,1,4096,b"".as_slice(),"48df0c462329cd879661bd05b39aa81b05cc16afd27a7196a559da83531d39d9"),
   (100000usize,1,4096,b"".as_slice(),"f2096a36c5cdca4fa33ee8852833150bb324992e5417a9d571f1bffff73b9efc"),
   (4096usize,1,4096,b"".as_slice(),"6ac39979016e3ddf3d39fff6cb984f7c118acdf1852919f5c100c4b142c1818e"),
   (1usize,1,4096,b"".as_slice(),"b803429503d95915829b29fdbc8bbad142f3abfd11b1cadf5526582e685c0551"),
   (1000000usize,1,4096,b"abcd".as_slice(),"917900b0d299454aa304d5debc6f39e4af7b5abe33bdbc568d5d8f1e5c4d8652"),
   (1000000usize,1,1024,b"".as_slice(),"e9df927c14fcb961d5f51c666d8ae4c14fe4ff98a374c733e898d00c9e74a8e3"),
   (1000000usize,1,65536,b"".as_slice(),"f3b6418f26d4d0e74728193bae76f15cb4bb2ce9777448d76bd8138b69ec61c2"),
   (1000000usize,2,4096,b"abcd".as_slice(),"8425c6d0c94f84ed904c12936845fbb7af9953753789712dcc3be142db3d4b6b47a399ad52aa609256ce29a960bf4bb0e595ec386ca58c06519d546dc5b197bb")]{
    let bytes=(0..size).map(|i|((i%11)+(i%439)+(i%1103)) as u8).collect::<Vec<_>>();
    let files=Data::new(&bytes);let proof=build(&files.data(),&files.tree(),512,BuildOptions::new(algorithm,block,salt.to_vec(),65536,65536).unwrap(),||false).unwrap();assert_eq!(hex(&proof.digest()),expected);
  }
 }
 #[test]
 fn streamed_multilevel_tree_is_root_first_and_every_leaf_authenticates(){
  for algorithm in [1,2]{
   let bytes=(0..4096*257+17).map(|i|(i%251)as u8).collect::<Vec<_>>();let files=Data::new(&bytes);let data=files.data();let tree=files.tree();
   let proof=build(&data,&tree,512,BuildOptions::new(algorithm,4096,vec![9],16384,4096).unwrap(),||false).unwrap();let layout=proof.levels().unwrap();assert!(layout.len()>1);assert_eq!(layout.last().unwrap().0,0);
   let fanout=4096/proof.options.digest_size();
   for index in 0..bytes.len().div_ceil(4096){let mut block=vec![0;4096];let n=(bytes.len()-index*4096).min(4096);block[..n].copy_from_slice(&bytes[index*4096..index*4096+n]);let mut hash=proof.options.hash(&block);let mut at=index;
    for(offset,_)in &layout{let mut node=vec![0;4096];read_exact(&tree,&mut node,512+offset+(at/fanout*4096)as u64).unwrap();assert_eq!(&node[at%fanout*hash.len()..at%fanout*hash.len()+hash.len()],hash);hash=proof.options.hash(&node);at/=fanout;}
    assert_eq!(hash,proof.root());
   }
   let mut bad=*proof.bytes();bad[112]=1;assert!(Descriptor::from_bytes(bad).is_err());
  }
 }
 #[test]
 fn fatal_interrupt_and_short_input_are_not_proofs(){
  let files=Data::new(&vec![7;8193]);let tree=files.tree();assert!(matches!(build(&files.data(),&tree,0,BuildOptions::new(1,4096,vec![],4096,4096).unwrap(),||true),Err(Error::Linux(4))));
  let file=files.data();let mut buffer=[0;4];assert!(matches!(read_exact(&file,&mut buffer,8192),Err(Error::Linux(EIO))));
  assert!(matches!(BuildOptions::new(1,2047,vec![],4096,4096),Err(Error::Linux(EINVAL))));
  assert!(matches!(BuildOptions::new(1,4096,vec![0;33],4096,4096),Err(Error::Linux(EMSGSIZE))));
  let mut arg=[0;128];arg[..4].copy_from_slice(&1u32.to_le_bytes());arg[4..8].copy_from_slice(&1u32.to_le_bytes());arg[8..12].copy_from_slice(&4096u32.to_le_bytes());
  assert!(BuildOptions::from_enable_arg(&arg,vec![],16384,4096).is_ok());arg[40]=1;assert!(matches!(BuildOptions::from_enable_arg(&arg,vec![],16384,4096),Err(Error::Linux(EINVAL))));arg[40]=0;
  arg[24..28].copy_from_slice(&16129u32.to_le_bytes());assert!(matches!(BuildOptions::from_enable_arg(&arg,vec![],16384,4096),Err(Error::Linux(EMSGSIZE))));

 }
}
