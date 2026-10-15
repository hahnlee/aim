use super::*;
use std::collections::{BTreeMap, BTreeSet};

/// An installed mount object, retained by its namespace until unmount.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MountRecord {
    pub id: u64,
    pub origin:u64,
    pub parent: u64,
    pub shared: u64,
    pub master: u64,
    pub kind: String,
    pub guest: String,
    pub host: String,
    pub source: String,
    pub fstype: String,
    pub root: String,
    pub own: bool,
}

fn allocate(owner: &Namespace, count: usize) -> io::Result<Vec<u64>> {
    let mut file = crate::private_fd::PrivateFile::allocate(|| fs::OpenOptions::new()
        .read(true).write(true).create(true).truncate(false)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC).open(owner.directory().join("mount-ids")))?;
    loop {
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } == 0 { break; }
        let failure = io::Error::last_os_error();
        if failure.kind() != io::ErrorKind::Interrupted { return Err(failure); }
    }
    let mut bytes = Vec::new(); file.read_to_end(&mut bytes)?;
    let old = if bytes.is_empty() { 0 } else { u64::from_le_bytes(bytes.as_slice().try_into().map_err(|_| protocol())?) };
    let next = old.checked_add(count as u64).filter(|next| *next <= i32::MAX as u64)
        .ok_or_else(|| io::Error::from_raw_os_error(libc::EOVERFLOW))?;
    use std::io::{Seek, SeekFrom};
    file.seek(SeekFrom::Start(0))?;file.write_all(&next.to_le_bytes())?;file.sync_all()?;
    Ok((old + 1..=next).collect())
}

fn below(parent: &str, child: &str) -> bool {
    parent == "/" && child.starts_with('/') || child.strip_prefix(parent).is_some_and(|rest| rest.starts_with('/'))
}

fn definition(line: &str, own: bool) -> io::Result<Option<MountRecord>> {
    let fields: Vec<_> = line.split('\t').collect();
    if fields.len() < 3 || !matches!(fields[0], "root" | "ro" | "rw" | "kernfs" | "cgroup2" | "bpf" | "input" | "host-device") { return Ok(None); }
    if !fields[1].starts_with('/') || !fields[2].starts_with('/') { return Err(protocol()); }
    Ok(Some(MountRecord { id: 0, origin:0, parent: 0, shared: 0, master: 0,
        kind: fields[0].into(), guest: fields[1].into(), host: fields[2].into(),
        source: fields.get(3).unwrap_or(&"").to_string(), fstype: fields.get(4).unwrap_or(&"").to_string(), root: "/".into(), own }))
}

fn insert(owner: &Namespace, mounts: &mut Vec<MountRecord>, mut record: MountRecord) -> io::Result<()> {
    record.id = allocate(owner, 1)?[0];
    record.parent = mounts.iter().filter(|mount| mount.guest == record.guest || below(&mount.guest, &record.guest))
        .max_by_key(|mount| (mount.guest.len(), mount.id)).map_or(record.id, |mount| mount.id);
    if let Some(parent) = mounts.iter().find(|mount| mount.id == record.parent) {
        if record.shared==0&&parent.shared!=0{record.shared=allocate(owner,1)?[0];}
    }
    mounts.push(record);Ok(())
}

pub(super) fn initialize(owner: &Namespace, base: &str) -> io::Result<Vec<MountRecord>> {
    let mut definitions = base.lines().filter_map(|line| definition(line, false).transpose()).collect::<io::Result<Vec<_>>>()?;
    definitions.sort_by_key(|mount| mount.guest.len());
    let mut mounts = Vec::new();for definition in definitions { insert(owner, &mut mounts, definition)?; }
    apply_policies(owner, &mut mounts, base)?;validate(&mounts)?;Ok(mounts)
}

pub(super) fn bind_base(owner:&Namespace,mounts:&mut Vec<MountRecord>,old_base:&str,base:&str,target:&str)->io::Result<()>{
    let previous=mounts.iter().filter(|mount|mount.guest==target).cloned().collect::<Vec<_>>();
    let old=previous.iter().max_by_key(|mount|mount.id).map(|mount|mount.id);
    replace_base(owner,mounts,old_base,base)?;
    let definition=base.lines().filter_map(|line|definition(line,false).transpose()).collect::<io::Result<Vec<_>>>()?.into_iter().find(|record|record.guest==target).ok_or_else(protocol)?;
    let latest=mounts.iter().filter(|mount|mount.guest==target&&mount.host==definition.host).max_by_key(|mount|mount.id).map(|mount|mount.id);
    for item in previous{if !mounts.iter().any(|mount|mount.id==item.id){mounts.push(item);}}
    if latest==old{insert(owner,mounts,definition)?;}
    if let Some(parent)=old{if let Some(mount)=mounts.iter_mut().filter(|mount|mount.guest==target&&mount.id!=parent).max_by_key(|mount|mount.id){mount.parent=parent;}}
    let rules=base.lines().filter(|line|line.starts_with(&format!("bind-source\t{target}\t"))||line.starts_with(&format!("propagation\t{target}\t"))).collect::<Vec<_>>().join("\n");
    apply_policies(owner,mounts,&rules)?;validate(mounts)
}

pub(super) fn replace_base(owner: &Namespace, mounts: &mut Vec<MountRecord>, old_base:&str, base: &str) -> io::Result<()> {
    let definitions = base.lines().filter_map(|line| definition(line, false).transpose()).collect::<io::Result<Vec<_>>>()?;
    mounts.retain(|mount| mount.own || definitions.iter().any(|item| item.guest == mount.guest && item.host == mount.host && item.kind == mount.kind));
    for definition in definitions {
        if !mounts.iter().any(|mount| !mount.own && mount.guest == definition.guest && mount.host == definition.host && mount.kind == definition.kind) {
            insert(owner, mounts, definition)?;
        }
    }
    let delta=base.lines().filter(|line|!old_base.lines().any(|old|old==*line)).collect::<Vec<_>>().join("\n");
    apply_policies(owner, mounts, &delta)?;repair_parents(mounts);validate(mounts)
}

fn repair_parents(mounts: &mut [MountRecord]) {
    let ids: BTreeSet<_> = mounts.iter().map(|mount| mount.id).collect();
    let snapshot = mounts.to_vec();
    for mount in mounts {
        if !ids.contains(&mount.parent) {
            mount.parent = snapshot.iter().filter(|other| other.id != mount.id && below(&other.guest, &mount.guest))
                .max_by_key(|other| (other.guest.len(), other.id)).map_or(mount.id, |other| other.id);
        }
    }
}

fn apply_policies(owner: &Namespace, mounts: &mut [MountRecord], event: &str) -> io::Result<()> {
    for line in event.lines() {
        let fields: Vec<_> = line.split('\t').collect();
        let rule = match fields.as_slice() {
            ["propagation", target, kind] => Some((*target, *kind, true)),
            ["propagation-flags", target, flags] => {
                let flags = flags.parse::<u64>().map_err(|_| protocol())?;
                Some((*target, if flags & (1 << 19) != 0 { "slave" } else if flags & (1 << 20) != 0 { "shared" } else { "private" }, flags & 0x4000 != 0))
            },_ => None,
        };
        if let Some((target, kind, recursive)) = rule {
            for mount in mounts.iter_mut().filter(|mount| mount.guest == target || recursive && below(target, &mount.guest)) {
                match kind {
                    "private" => { mount.shared = 0;mount.master = 0; },
                    "slave" => { if mount.shared != 0 { mount.master = mount.shared; }mount.shared = 0; },
                    "shared" => { if mount.shared == 0 { mount.shared = allocate(owner, 1)?[0]; } },
                    _ => return Err(protocol()),
                }
            }
        }
    }
    for line in event.lines(){
        let fields:Vec<_>=line.split('\t').collect();
        if let ["bind-source", target, source] = fields.as_slice() {
            let origin=mounts.iter().filter(|mount|mount.guest!=*target&&(mount.guest==*source||below(&mount.guest,source))).max_by_key(|mount|(mount.guest.len(),mount.id)).cloned();
            if let Some(mount) = mounts.iter_mut().rev().find(|mount| mount.guest == *target) {
                if let Some(origin)=origin{
                    let suffix=&source[origin.guest.trim_end_matches('/').len()..];mount.root=format!("{}{suffix}",origin.root.trim_end_matches('/'));if mount.root.is_empty(){mount.root="/".into();}mount.shared=origin.shared;mount.master=origin.master;
                    if let Some(rule)=event.lines().filter_map(|line|{let fields:Vec<_>=line.split('\t').collect();match fields.as_slice(){["propagation",path,kind]if *path==*target=>Some((*kind).to_owned()),_=>None}}).last(){match rule.as_str(){"private"=>{mount.shared=0;mount.master=0;},"slave"=>{mount.shared=0;mount.master=origin.shared;},_=>{}}}
                }else{return Err(protocol());}
            }
        }
    }
    Ok(())
}

pub(super) fn apply(owner: &Namespace, mounts: &mut Vec<MountRecord>, event: &str) -> io::Result<()> {
    for line in event.lines() {
        if let Some(target) = line.strip_prefix("remove\t") {
            if let Some(index) = mounts.iter().rposition(|mount| mount.own && mount.guest == target) { mounts.remove(index); }
        } else if let Some(rest) = line.strip_prefix("move\t") {
            let (from, to) = rest.split_once('\t').ok_or_else(protocol)?;
            for mount in mounts.iter_mut().filter(|mount| mount.own && (mount.guest == from || below(from, &mount.guest))) {
                mount.guest = format!("{to}{}", &mount.guest[from.len()..]);
            }
        } else if let Some(record) = definition(line, true)? { insert(owner, mounts, record)?; }
    }
    apply_policies(owner, mounts, event)?;repair_parents(mounts);validate(mounts)
}

pub(super) fn clone_mounts(owner: &Namespace, mounts: &mut [MountRecord]) -> io::Result<()> {
    let ids = allocate(owner, mounts.len())?;
    let translations: BTreeMap<_, _> = mounts.iter().map(|mount| mount.id).zip(ids).collect();
    for mount in mounts { mount.parent = *translations.get(&mount.parent).ok_or_else(protocol)?;mount.id = translations[&mount.id]; }
    Ok(())
}

pub(super) fn validate(mounts: &[MountRecord]) -> io::Result<()> {
    let ids: BTreeSet<_> = mounts.iter().map(|mount| mount.id).collect();
    if ids.len() != mounts.len() || mounts.iter().any(|mount| mount.id == 0 || mount.id > i32::MAX as u64 || !ids.contains(&mount.parent)) { return Err(protocol()); }
    Ok(())
}


pub(super) fn sync_projections(owner:&Namespace,mounts:&mut Vec<MountRecord>,live:&[MountRecord])->io::Result<bool>{
    let same=|a:&MountRecord,b:&MountRecord|a.guest==b.guest&&a.host==b.host&&(a.kind!="projected"||a.origin==b.origin);
    let before=mounts.len();mounts.retain(|mount|mount.kind!="projected"||live.iter().any(|item|same(mount,item)));let mut changed=before!=mounts.len();
    for item in live{if !mounts.iter().any(|mount|same(mount,item)){insert(owner,mounts,item.clone())?;changed=true;}}
    repair_parents(mounts);validate(mounts)?;Ok(changed)
}

pub(super) fn propagated_group(owner:&Namespace,source:u64,parent_group:u64)->io::Result<u64>{
    if source==0||parent_group==0{return Err(protocol());}
    let mut file=crate::private_fd::PrivateFile::allocate(||fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).custom_flags(libc::O_NOFOLLOW|libc::O_CLOEXEC).open(owner.directory().join(format!("propagate-{source}-{parent_group}"))))?;
    loop{if unsafe{libc::flock(file.as_raw_fd(),libc::LOCK_EX)}==0{break;}let error=io::Error::last_os_error();if error.kind()!=io::ErrorKind::Interrupted{return Err(error);}}
    let mut bytes=Vec::new();file.read_to_end(&mut bytes)?;
    if !bytes.is_empty(){let group=u64::from_le_bytes(bytes.as_slice().try_into().map_err(|_|protocol())?);if group==0||group>i32::MAX as u64{return Err(protocol());}return Ok(group);}
    let group=allocate(owner,1)?[0];file.write_all(&group.to_le_bytes())?;file.sync_all()?;Ok(group)
}
