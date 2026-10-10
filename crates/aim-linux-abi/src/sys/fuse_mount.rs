//! Linux fuse mount option grammar; session creation belongs to /dev/fuse.
use crate::errno::{Errno,EINVAL};
#[derive(Clone,Debug,PartialEq,Eq)]
pub struct Options {
    pub fd:i32,pub root_mode:u32,pub uid:u32,pub gid:u32,
    pub default_permissions:bool,pub allow_other:bool,pub max_read:Option<u32>,
}
pub fn parse(data:&[u8])->Result<Options,Errno>{
    let text=std::str::from_utf8(data).map_err(|_|EINVAL)?;
    let(mut fd,mut root,mut uid,mut gid,mut max_read)=(None,None,None,None,None);
    let(mut default_permissions,mut allow_other)=(false,false);
    for option in text.split(','){
        match option {
            "default_permissions"=>default_permissions=true,"allow_other"=>allow_other=true,
            // Linux fs/fs_context.c vfs_parse_monolithic_sep ignores empty tokens.
            ""=>continue,
            value=>{
                let(key,value)=value.split_once('=').ok_or(EINVAL)?;
                match key{
                    "fd"=>{let number=value.parse::<i32>().map_err(|_|EINVAL)?;if number<0{return Err(EINVAL);}fd=Some(number);},
                    "rootmode"=>root=Some(u32::from_str_radix(value,8).map_err(|_|EINVAL)?),
                    "user_id"=>uid=Some(value.parse::<u32>().map_err(|_|EINVAL)?),
                    "group_id"=>gid=Some(value.parse::<u32>().map_err(|_|EINVAL)?),
                    "max_read"=>max_read=Some(value.parse::<u32>().map_err(|_|EINVAL)?),
                    // fuseblk-only block size is not a fuse mount option.
                    _=>return Err(EINVAL),
                }
            }
        }
    }
    let root_mode=root.ok_or(EINVAL)?;
    if root_mode&0o170000!=0o040000{return Err(EINVAL);}
    Ok(Options{fd:fd.ok_or(EINVAL)?,root_mode,uid:uid.ok_or(EINVAL)?,gid:gid.ok_or(EINVAL)?,default_permissions,allow_other,max_read})
}
#[cfg(test)]mod tests{use super::*;#[test]fn actual_vold_options_and_required_identity(){
    let options=parse(b"fd=17,rootmode=40000,user_id=0,group_id=0,default_permissions,allow_other,max_read=131072").unwrap();
    assert_eq!(options.fd,17);assert!(options.default_permissions&&options.allow_other);
    let original=parse(b"fd=37,rootmode=40000,allow_other,user_id=0,group_id=0,").unwrap();
    assert_eq!(original.fd,37);assert!(original.allow_other);assert!(!original.default_permissions);
    assert_eq!(parse(b",fd=37,,rootmode=40000,allow_other,user_id=0,group_id=0,,").unwrap(),original);
    assert!(parse(b"fd=17,rootmode=100000,user_id=0,group_id=0").is_err());
    assert!(parse(b"fd=-1,rootmode=40000,user_id=0,group_id=0").is_err());
    assert!(parse(b"fd=17,rootmode=40000,user_id=0").is_err());
}}
