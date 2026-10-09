//! Native init's bootstrap/default mount owners (pinned init/apexd, #1228).
use crate::{apex::{self, ApexEntry}, paths::{Layout, MapEntry, MapKind, PathMap}};
use aim_storage::{mount_namespace::Namespace, process_namespace::{self, ProcessIdentity}};
use std::{collections::BTreeSet, fs, path::PathBuf};

pub struct MountNamespaces {
    bootstrap: Namespace,
    default: Namespace,
    process: ProcessIdentity,
    table: PathBuf,
    path_map: PathBuf,
}

impl MountNamespaces {
    pub fn setup(layout: &Layout, map: &PathMap, entries: &[ApexEntry], early_vm: bool) -> Result<(Self, PathMap), String> {
        let process = ProcessIdentity::running(unsafe { libc::getpid() }).map_err(|error| error.to_string())?;
        let table = layout.identity_dir().join("by-pid");
        let init = process_namespace::InitRegistration::read(&table).map_err(|error| error.to_string())?;
        if init.process != process { return Err("mount namespace setup belongs to another init".into()); }
        let bootstrap = Namespace::open(&layout.runtime, &init.mount_namespace).map_err(|error| error.to_string())?;
        let mut base = map.clone();
        base.add(MapEntry{guest:"/apex".into(),host:map.lookup("/apex").0,kind:MapKind::ReadOnly});
        for entry in entries {
            let guest=format!("/apex/{}",entry.module_name);
            let source=map.lookup(&guest).0;
            if !source.is_dir(){return Err(format!("activated APEX {} is absent",entry.module_name));}
            base.add(MapEntry{guest,host:source,kind:MapKind::ReadOnly});
        }
        base.propagation("/", "shared", true);
        base.propagation("/apex", "private", true);
        base.propagation("/linkerconfig", "private", true);
        for target in ["/mnt/installer", "/mnt/androidwritable"] {
            let host = base.lookup("/mnt/user").0;
            base.add(MapEntry { guest: target.into(), host, kind: MapKind::Writable });
            base.bind_source(target, "/mnt/user");
            base.propagation(target, "slave", false);
        }
        bootstrap.update_base(&base.to_file_text()).map_err(|error| error.to_string())?;
        for target in ["/mnt/installer","/mnt/androidwritable"]{bootstrap.append(&format!("propagation-flags\t{target}\t{}",1u64<<20)).map_err(|error|error.to_string())?;}
        let default = bootstrap.clone_to(&format!("default-{}-{}-{}", process.host_pid, process.start_seconds, process.start_microseconds)).map_err(|error| error.to_string())?;
        let view = layout.runtime.join("bootstrap-apex-view");
        fs::create_dir(&view).map_err(|error| error.to_string())?;
        let names: BTreeSet<_> = entries.iter().filter(|entry| entry.vendor_bootstrap
            || matches!(entry.module_name.as_str(), "com.android.runtime" | "com.android.i18n" | "com.android.tzdata")
            || early_vm && entry.module_name == "com.android.virt").map(|entry| entry.module_name.clone()).collect();
        let selected: Vec<_> = entries.iter().filter(|entry| names.contains(&entry.module_name)).cloned().collect();
        for required in ["com.android.runtime","com.android.i18n","com.android.tzdata"].into_iter().chain(early_vm.then_some("com.android.virt")){
            if !names.contains(required){return Err(format!("pinned bootstrap APEX {required} is absent"));}
        }
        fs::write(view.join("apex-info-list.xml"), apex::apex_info_list_xml(&selected)).map_err(|error| error.to_string())?;
        let mut bootstrap_map = base.clone();
        bootstrap_map.retain_entries(|entry|!entry.guest.starts_with("/apex/"));
        bootstrap_map.add(MapEntry{guest:"/apex/apex-info-list.xml".into(),host:view.join("apex-info-list.xml"),kind:MapKind::ReadOnly});
        for target in ["/apex", "/bootstrap-apex"] {
            bootstrap_map.add(MapEntry { guest: target.into(), host: view.clone(), kind: MapKind::ReadOnly });
        }
        for entry in selected {
            let source = map.lookup(&format!("/apex/{}", entry.module_name)).0;
            if !source.is_dir() { return Err(format!("bootstrap APEX {} is not activated", entry.module_name)); }
            std::os::unix::fs::symlink(format!("/bootstrap-apex/{}", entry.module_name), view.join(&entry.module_name)).map_err(|error| error.to_string())?;
            for target in [format!("/apex/{}", entry.module_name), format!("/bootstrap-apex/{}", entry.module_name)] {
                bootstrap_map.add(MapEntry { guest: target, host: source.clone(), kind: MapKind::ReadOnly });
            }
        }
        bootstrap_map.propagation("/", "shared", true);
        bootstrap_map.propagation("/apex", "private", true);
        bootstrap_map.propagation("/linkerconfig", "private", true);
        let mut default_map = base;
        default_map.add(MapEntry { guest: "/bootstrap-apex".into(), host: view, kind: MapKind::ReadOnly });
        for entry in bootstrap_map.entries().iter().filter(|entry| entry.guest.starts_with("/bootstrap-apex/")) { default_map.add(entry.clone()); }
        default.update_base(&default_map.to_file_text()).map_err(|error| error.to_string())?;
        bootstrap.update_base(&bootstrap_map.to_file_text()).map_err(|error| error.to_string())?;
        let owner = Self { bootstrap, default, process, table, path_map: layout.path_map_file() };
        owner.publish_current(&bootstrap_map, owner.bootstrap.id())?;
        Ok((owner, bootstrap_map))
    }

    fn current(&self) -> Result<Namespace, String> {
        let id = process_namespace::mount_namespace_of(&self.table, self.process).map_err(|error| error.to_string())?;
        Namespace::open(self.bootstrap.runtime(), &id).map_err(|error| error.to_string())
    }

    fn publish_current(&self, map: &PathMap, id: &str) -> Result<(), String> {
        let temporary = self.path_map.with_extension("mount-next");
        fs::write(&temporary, map.to_file_text()).map_err(|error| error.to_string())?;
        fs::rename(&temporary, &self.path_map).map_err(|error| error.to_string())?;
        process_namespace::register_mount_namespace(&self.table, self.process, id).map_err(|error| error.to_string())
    }

    pub fn publish_map(&self, map: &PathMap) -> Result<(), String> {self.publish(map,None)}
    pub fn publish_bind(&self,map:&PathMap,target:&str)->Result<(),String>{self.publish(map,Some(target))}
    fn publish(&self,map:&PathMap,bound:Option<&str>)->Result<(),String>{
        let current = self.current()?;
        let before=current.read().map_err(|error|error.to_string())?;
        let prior=PathMap::parse_file_text(&before.base)?;
        let changed:Vec<_>=map.entries().iter().filter(|entry|!prior.entries().contains(entry)).cloned().collect();
        if let Some(target)=bound{current.bind_base(&map.to_file_text(),target).map_err(|error|error.to_string())?;}else{current.update_base(&map.to_file_text()).map_err(|error|error.to_string())?;}
        let sibling=if current.id()==self.bootstrap.id(){&self.default}else if current.id()==self.default.id(){&self.bootstrap}else{return Err("native mount publisher is outside its init owners".into());};
        let other=sibling.read().map_err(|error|error.to_string())?;
        let mut sibling_map=PathMap::parse_file_text(&other.base)?;
        for entry in changed{
            let parent=before.mounts.iter().filter(|mount|prefix(&mount.guest,&entry.guest)).max_by_key(|mount|(mount.guest.len(),mount.id));
            let target=other.mounts.iter().filter(|mount|prefix(&mount.guest,&entry.guest)).max_by_key(|mount|(mount.guest.len(),mount.id));
            if let(Some(parent),Some(target))=(parent,target)&&parent.shared!=0&&(target.shared==parent.shared||target.master==parent.shared){let target=entry.guest.clone();sibling_map.add(entry);if let Some(source)=map.bind_source_of(&target){sibling_map.bind_source(&target,source);}}
        }
        if let Some(target)=bound&&sibling_map.lookup(target).0==map.lookup(target).0{ sibling.bind_base(&sibling_map.to_file_text(),target).map_err(|error|error.to_string())?;}else{sibling.update_base(&sibling_map.to_file_text()).map_err(|error|error.to_string())?;}
        self.publish_current(map, current.id())
    }

    pub fn enter_default(&self) -> Result<PathMap, String> {
        let current = self.current()?;
        if current.id() != self.bootstrap.id() && current.id() != self.default.id() {
            return Err("native init is in an unrelated private mount namespace".into());
        }
        let state = self.default.read().map_err(|error| error.to_string())?;
        let mut map = PathMap::parse_file_text(&state.base)?;
        let source = map.lookup("/linkerconfig/default").0;
        if !source.is_dir() { return Err("default linkerconfig directory is absent".into()); }
        map.add(MapEntry { guest: "/linkerconfig".into(), host: source, kind: MapKind::Writable });
        map.bind_source("/linkerconfig", "/linkerconfig/default");
        map.propagation("/linkerconfig", "private", true);
        self.default.update_base(&map.to_file_text()).map_err(|error| error.to_string())?;
        self.publish_current(&map, self.default.id())?;
        Ok(map)
    }

    pub fn namespace_ids(&self) -> (&str, &str) { (self.bootstrap.id(), self.default.id()) }
}

fn prefix(parent:&str,child:&str)->bool{parent==child||parent=="/"&&child.starts_with('/')||child.strip_prefix(parent).is_some_and(|rest|rest.starts_with('/'))}

#[cfg(test)]
mod tests{
    use super::*;
    use crate::fsops::FsOps;
    #[test]
    #[ignore="real child invoked by bootstrap/default namespace owner test"]
    fn namespace_child_view(){
        use std::io::BufRead;
        let fields=std::env::args().find_map(|arg|arg.strip_prefix("NATIVE_MOUNT_OWNER=").map(str::to_owned)).expect("parent namespace receipt arguments");let fields:Vec<_>=fields.split('|').collect();
        let mut line=String::new();std::io::BufReader::new(std::io::stdin()).read_line(&mut line).unwrap();
        let process=ProcessIdentity::running(unsafe{libc::getpid()}).unwrap();let id=process_namespace::mount_namespace_of(std::path::Path::new(fields[0]),process).unwrap();
        let state=Namespace::open(std::path::Path::new(fields[1]),&id).unwrap().read().unwrap();let map=PathMap::parse_file_text(&state.base).unwrap();
        assert_eq!(fs::read(map.resolve("/data/shared-view/marker",true).unwrap().host).unwrap(),b"shared actual mount");
        assert_eq!(fs::read_to_string(map.resolve("/apex/com.android.runtime/content",true).unwrap().host).unwrap(),"com.android.runtime");
        assert_eq!(fs::read(map.resolve("/apex/example.regular/content",true).unwrap().host).unwrap_err().kind(),std::io::ErrorKind::NotFound);
        println!("NAMESPACE_CHILD_ACTUAL_VIEW_EXECUTED");
    }

    #[test]
    fn real_bootstrap_default_views_private_apex_and_shared_mounts(){
        let root=std::env::temp_dir().join(format!("aim-init-namespaces-{}",std::process::id()));let _=fs::remove_dir_all(&root);
        let layout=Layout{image:root.join("image"),data:root.join("data"),runtime:root.join("run")};
        for dir in [&layout.image,&layout.data,&layout.runtime,&layout.identity_dir().join("by-pid"),&layout.runtime.join("linkerconfig/default"),&layout.runtime.join("linkerconfig/bootstrap"),&layout.data.join("alternate")]{fs::create_dir_all(dir).unwrap();}
        let entries:Vec<_>=["com.android.runtime","com.android.i18n","com.android.tzdata","com.android.virt","example.vendor","example.regular"].into_iter().map(|name|{
            fs::create_dir_all(layout.image.join("apex").join(name)).unwrap();fs::write(layout.image.join("apex").join(name).join("content"),name).unwrap();
            ApexEntry{module_name:name.into(),module_path:format!("/system/apex/{name}.apex"),version_code:1,version_name:"one".into(),partition:"SYSTEM".into(),vendor_bootstrap:name=="example.vendor"}
        }).collect();
        let process=ProcessIdentity::running(unsafe{libc::getpid()}).unwrap();let table=layout.identity_dir().join("by-pid");
        let base=layout.path_map();let data=base.lookup("/data").0;fs::create_dir_all(data.join("alternate")).unwrap();let bootstrap=Namespace::open(&layout.runtime,"actual-bootstrap").unwrap();bootstrap.initialize(&base.to_file_text()).unwrap();
        process_namespace::InitRegistration::register(&table,process,bootstrap.id()).unwrap();process_namespace::register_mount_namespace(&table,process,bootstrap.id()).unwrap();
        let(owner,map)=MountNamespaces::setup(&layout,&base,&entries,true).unwrap();let default=owner.default.clone();let bootstrap=owner.bootstrap.clone();
        let before=bootstrap.read().unwrap();assert_ne!(before.mounts[0].id,default.read().unwrap().mounts[0].id);
        let default_before=default.read().unwrap();
        for entry in &entries {
            let guest=format!("/apex/{}",entry.module_name);
            let mount=default_before.mounts.iter().find(|mount|mount.guest==guest).unwrap();
            assert_eq!(mount.kind,"ro");assert_eq!(PathBuf::from(&mount.host),layout.image.join("apex").join(&entry.module_name));
        }

        assert_eq!(fs::read(map.resolve("/apex/example.regular/content",true).unwrap().host).unwrap_err().kind(),std::io::ErrorKind::NotFound);assert_eq!(fs::read_to_string(map.resolve("/apex/com.android.virt/content",true).unwrap().host).unwrap(),"com.android.virt");
        use std::io::Write;
        let marker=format!("NATIVE_MOUNT_OWNER={}|{}",table.display(),layout.runtime.display());
        let child=std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact","mount_namespace::tests::namespace_child_view","--ignored","--nocapture","--skip",&marker]).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).spawn().unwrap();
        struct Child(std::process::Child);impl Drop for Child{fn drop(&mut self){if self.0.try_wait().ok().flatten().is_none(){let _=self.0.kill();}let _=self.0.wait();}}
        let mut child=Child(child);let child_process=ProcessIdentity::running(child.0.id()as i32).unwrap();process_namespace::register_mount_namespace(&table,child_process,bootstrap.id()).unwrap();
        let mut ops=FsOps::new(map,root.join("attrs"),true);ops.set_path_map_file(layout.path_map_file());ops.set_mount_namespaces(owner);
        ops.enter_default_mount_namespace().unwrap();assert_eq!(process_namespace::mount_namespace_of(&table,process).unwrap(),default.id());assert_eq!(process_namespace::InitRegistration::read(&table).unwrap().mount_namespace,bootstrap.id());
        assert_eq!(fs::read_to_string(ops.map.resolve("/apex/example.regular/content",true).unwrap().host).unwrap(),"example.regular");
        assert_eq!(ops.map.resolve("/apex/example.regular/content",true).unwrap().area,crate::paths::Area::ReadOnlyImage);
        for entry in &entries {
            let guest=format!("/apex/{}",entry.module_name);
            let previous=default_before.mounts.iter().find(|mount|mount.guest==guest).unwrap();
            assert_eq!(default.read().unwrap().mounts.iter().find(|mount|mount.guest==guest).unwrap().id,previous.id);
        }

        fs::create_dir_all(data.join("shared-view")).unwrap();ops.mount("none","/data/alternate","/data/shared-view",&["bind".into()]).unwrap();
        let shared=PathMap::parse_file_text(&bootstrap.read().unwrap().base).unwrap();assert_eq!(shared.lookup("/data/shared-view").0,data.join("alternate"));
        ops.mount("none","/data/alternate","/apex/com.android.runtime",&["bind".into()]).unwrap();
        let private=PathMap::parse_file_text(&bootstrap.read().unwrap().base).unwrap();assert_eq!(private.lookup("/apex/com.android.runtime").0,layout.image.join("apex/com.android.runtime"));
        fs::write(data.join("alternate/marker"),b"shared actual mount").unwrap();child.0.stdin.take().unwrap().write_all(b"published\n").unwrap();
        let deadline=std::time::Instant::now()+std::time::Duration::from_secs(5);loop{if let Some(status)=child.0.try_wait().unwrap(){assert!(status.success());break;}if std::time::Instant::now()>=deadline{panic!("owned namespace child timed out");}std::thread::sleep(std::time::Duration::from_millis(5));}
        let mut output=String::new();use std::io::Read;child.0.stdout.take().unwrap().read_to_string(&mut output).unwrap();assert!(output.contains("NAMESPACE_CHILD_ACTUAL_VIEW_EXECUTED"));
        assert_eq!(before.mounts[0].id,bootstrap.read().unwrap().mounts[0].id);fs::remove_dir_all(root).unwrap();
    }
}
