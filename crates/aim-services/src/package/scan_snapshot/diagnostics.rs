//! Original PMS diagnostic owners rendered from one retained native capture.
//! DumpState/Settings/PackageSetting, android-16.0.0_r1 (AOSP Apache 2.0).
use super::query_state::Capture;
use crate::package::{apps_filter::NotModelled, model::PackageState};
use aim_binder_host::parcel::{Parcel, Reader};
use std::fmt::Write;

#[derive(Default)]
struct Options {
    title: bool,
    target: Option<String>,
    full: bool,
    brief: bool,
    shared: Option<(i32, String)>,
}
impl Options {
    fn read(bytes: Option<&[u8]>) -> Result<Self, NotModelled> {
        let Some(bytes) = bytes else { return Ok(Self::default()); };
        let mut r = Reader::new(bytes, &[]);
        let invalid = |_| NotModelled("diagnostic DumpState record malformed");
        if r.read_i32().map_err(invalid)? != 1 { return Err(NotModelled("diagnostic DumpState record version")); }
        let _types = r.read_i32().map_err(invalid)?;
        let _options = r.read_i32().map_err(invalid)?;
        let title = r.read_bool().map_err(invalid)?;
        let target = r.read_string16().map_err(invalid)?;
        let full = r.read_bool().map_err(invalid)?;
        let _checkin = r.read_bool().map_err(invalid)?;
        let brief = r.read_bool().map_err(invalid)?;
        let shared = if r.read_bool().map_err(invalid)? {
            Some((r.read_i32().map_err(invalid)?, r.read_string16().map_err(invalid)?.ok_or(NotModelled("diagnostic shared user name absent"))?))
        } else { None };
        if r.remaining() != 0 { return Err(NotModelled("diagnostic DumpState record trailing data")); }
        Ok(Self { title, target, full, brief, shared })
    }
}
enum Op { Start(i32,u64), End(i32), Int(u64,i32), Long(u64,i64), String(u64,String), Bool(u64,bool) }
fn message(number:u64)->u64 {0x20b00000000|number}
fn string(number:u64)->u64 {0x10900000000|number}
fn integer(number:u64)->u64 {0x10500000000|number}
fn long(number:u64)->u64 {0x10300000000|number}
fn boolean(number:u64)->u64 {0x10800000000|number}
fn selected<'a>(capture:&'a Capture,name:Option<&str>)->Vec<&'a PackageState> {
    capture.state().packages.values().filter(|p|name.is_none_or(|n|n==p.name)).collect()
}
fn packages(capture:&Capture,name:Option<&str>,checkin:bool,brief:bool,text:&mut String)->Result<(),NotModelled> {
    for p in selected(capture,name) {
        if checkin {let _=writeln!(text,"pkg,{},{},{},{}",p.name,p.app_id,p.version_code,p.path);continue;}
        let _=writeln!(text,"  Package [{}]:\n    userId={}\n    codePath={}",p.name,p.app_id,p.path);
        crate::package::dump::sdk_versions(text, p);
        if brief {continue;}
        let _=writeln!(text,"    flags={:?}\n    installSource={:?}",p.is,p.install_source);
        let dates = capture.state().system.diagnostic_dates.as_deref();
        crate::package::dump::timestamp(text,"    ","timeStamp",p.last_modified_time,dates).map_err(|_| NotModelled("diagnostic package date owner unavailable"))?;
        crate::package::dump::timestamp(text,"    ","lastUpdateTime",p.last_update_time,dates).map_err(|_| NotModelled("diagnostic package date owner unavailable"))?;
        for (user,u) in &p.users {let _=writeln!(text,"    User {}: installed={} hidden={} suspended={} stopped={} instant={} enabled={}",user,u.installed,u.hidden,u.suspensions.as_ref().is_some_and(|s|!s.is_empty()) || !u.suspended_by.is_empty(),u.stopped,u.instant_app,u.enabled);crate::package::dump::timestamp(text,"      ","firstInstallTime",u.first_install_time,dates).map_err(|_| NotModelled("diagnostic package date owner unavailable"))?;}
        for permission in &p.installed_permissions {let _=writeln!(text,"    permission={permission}");}
    }
    Ok(())
}
impl Capture {
    fn user_permission_proto(&self, package: &str, user: i32) -> Result<Vec<String>, NotModelled> {
        let state = self.scan().owner().legacy_permissions(package, false)
            .map_err(|_| NotModelled("diagnostic live legacy permission owner unavailable"))?
            .ok_or(NotModelled("diagnostic package legacy permission state absent"))?;
        let user = state.user(user).ok_or(NotModelled("diagnostic permission user outside capture"))?;
        Ok(user.permissions.iter().filter(|permission| permission.granted)
            .filter_map(|permission| permission.name.clone()).collect())
    }

    fn preferred_dump_xml(&self, user: i32, full: bool) -> Result<Vec<u8>, NotModelled> {
        let owner = self.state().system.preferred_owner.as_ref()
            .ok_or(NotModelled("diagnostic preferred registry unavailable"))?;
        let state = owner.captured.user_states().into_iter().find(|(id, _)| *id == user)
            .map(|(_, state)| state);
        let order = owner.captured.record_order(user, crate::package::preferred::records::PREFERRED)
            .map_err(|_| NotModelled("diagnostic preferred identity order unavailable"))?;
        let empty = crate::package::preferred::Preferred::default();
        state.as_deref().unwrap_or(&empty).preferred_dump_xml(&order, full)
            .map_err(|_| NotModelled("diagnostic preferred XML serialization failed"))
    }

    /// Protocol 2 is consumed by NativePMInterfaceProducerGroups.Diagnostics.
    pub fn diagnostic_record(&self,kind:i32,dump_type:i32,name:Option<&str>,permissions:Option<&[Option<String>]>,checkin:bool,state:Option<&[u8]>)->Result<Vec<u8>,NotModelled> {
        let mut options=Options::read(state)?;
        let name=name.or(options.target.as_deref());
        let mut text=String::new();let mut fd=None;let mut ops=Vec::new();
        let mut title=|label:&str,text:&mut String| {if options.title{text.push('\n');}let _=writeln!(text,"{label}:");options.title=true;};
        match kind {
            0 => match dump_type {
                32768 => {title("Database versions",&mut text);for v in &self.scan().owner().settings.versions {let _=writeln!(text,"  {}: sdkVersion={} databaseVersion={} buildFingerprint={}",v.volume_uuid.as_deref().unwrap_or("Internal"),v.sdk_version,v.database_version,v.build_fingerprint.as_deref().unwrap_or("null"));}},
                1 => {title("Libraries",&mut text);let libraries=self.state().shared_libraries.as_ref().ok_or(NotModelled("diagnostic shared library owner absent"))?;for l in libraries{let _=writeln!(text,"  {} type={} version={} path={}",l.name.as_deref().unwrap_or("null"),l.kind,l.version,l.path.as_deref().unwrap_or("null"));}},
                4096 => {title("Preferred Activities",&mut text);for (user,u) in &self.state().users {if let Some(xml)=&u.preferred_activities {let _=writeln!(text,"  User {user}:\n{}",String::from_utf8_lossy(xml));}}},
                8192 => {fd=Some(self.preferred_dump_xml(0,options.full)?);},
                524288 => {title("Frozen packages",&mut text);let values=self.frozen_packages()?;if values.is_empty(){text.push_str("  (none)\n");}else{for (p,count) in values{let _=writeln!(text,"  package={p}, refCounts={count}");}}},
                262144 => {title("Domain verification status",&mut text);for p in selected(self,name){if let Some((id,domains))=&p.domain_verification {let _=writeln!(text,"  {} id={id}",p.name);for(domain,status)in domains{let _=writeln!(text,"    {domain}: {status}");}}}},
                67108864 => {title("Queries",&mut text);for p in selected(self,name){if let Some(code)=&p.pkg{let _=writeln!(text,"  {} appId={} queriesPackages={:?} queriesProviders={:?}",p.name,p.app_id,code.queries_packages,code.queries_providers);}}},
                536870912 => {title("Snapshot statistics",&mut text);let _=writeln!(text,"  version={} packages={} sharedUsers={} uidSlots={}",self.scan().version(),self.state().packages.len(),self.state().shared_users.len(),self.state().uid_owners.as_ref().ok_or(NotModelled("diagnostic UID owner absent"))?.len());},
                33554432 => {title("APEX packages",&mut text);for p in self.state().packages.values().filter(|p|p.is.apex){let _=writeln!(text,"  {} version={} path={}",p.name,p.version_code,p.path);}},
                512 => {title("Settings parse messages",&mut text);text.push_str(&self.settings_read_messages()?);},
                1048576 | 2097152 => {text.push_str(&self.maintenance_diagnostic_text(dump_type,name)?);},
                _ => {},
            },
            1 => {title("Permissions",&mut text);for permission in self.scan().owner().settings.permissions.iter().chain(&self.scan().owner().settings.permission_trees) {if name.is_some_and(|name|name!=permission.package)||permissions.is_some_and(|names|!names.iter().any(|name|name.as_deref()==Some(permission.name.as_str()))){continue;}let _=writeln!(text,"  Permission [{}]: sourcePackage={} protectionLevel=0x{:x} owner={:?}",permission.name,permission.package,permission.protection_level,permission.owner);}},
            2 => {if !checkin{title("Packages",&mut text);}packages(self,name,checkin,options.brief,&mut text)?;},
            3 => {title("Key Set Manager",&mut text);let pools=&self.scan().owner().settings.key_sets;let _=writeln!(text,"  {:?}",pools);for p in selected(self,name){let _=writeln!(text,"  {} signing={:?}",p.name,p.signatures);}},
            4 => {if !checkin{title("Shared users",&mut text);}for group in self.state().shared_users.values(){if options.shared.as_ref().is_some_and(|(id,_)|*id!=group.app_id){continue;}if name.is_some_and(|name|!group.packages.iter().any(|p|p==name)){continue;}let _=writeln!(text,"  SharedUser [{}]: userId={} flags={} privateFlags={} packages={:?}",group.name,group.app_id,group.flags,group.private_flags,group.packages);}},
            5 => {for (i,g) in self.state().shared_users.values().enumerate(){let token=i as i32;ops.push(Op::Start(token,message(6)));ops.push(Op::Int(integer(1),g.app_id));ops.push(Op::String(string(2),g.name.clone()));ops.push(Op::End(token));}},
            6 => {
                let mut next=0i32;
                for p in self.state().packages.values() {
                    let package=next;next+=1;ops.push(Op::Start(package,message(5)));
                    ops.push(Op::String(string(1),p.real_name.clone().flatten().unwrap_or_else(||p.name.clone())));
                    ops.push(Op::Int(integer(2),p.app_id));ops.push(Op::Long(integer(3),p.version_code));
                    ops.push(Op::Long(long(6),p.last_update_time));
                    if let Some(installer)=&p.install_source.installer{ops.push(Op::String(string(7),installer.clone()));}
                    if let Some(code)=p.pkg.as_deref() {
                        if let Some(version)=&code.version_name{ops.push(Op::String(string(4),version.clone()));}
                        let split=next;next+=1;ops.push(Op::Start(split,message(8)));ops.push(Op::String(string(1),"base".into()));ops.push(Op::Int(integer(2),code.base_revision_code));ops.push(Op::End(split));
                        if let Some(names)=&code.split_names{for (index,name) in names.iter().enumerate(){
                            let split=next;next+=1;ops.push(Op::Start(split,message(8)));
                            if let Some(name)=name{ops.push(Op::String(string(1),name.clone()));}
                            let revisions=code.split_revision_codes.as_ref().ok_or(NotModelled("package proto split revision owner absent"))?;
                            ops.push(Op::Int(integer(2),*revisions.get(index).ok_or(NotModelled("package proto split revision count differs"))?));ops.push(Op::End(split));
                        }}
                        let source=next;next+=1;ops.push(Op::Start(source,0x10b0000000a));
                        for (field,value) in [(1,&p.install_source.initiating_package),(2,&p.install_source.originating_package),(3,&p.install_source.update_owner)]{
                            if let Some(value)=value{ops.push(Op::String(string(field),value.clone()));}
                        }ops.push(Op::End(source));
                    }
                    ops.push(Op::Bool(boolean(2),p.is.loading));
                    for (user,u) in &p.users {
                        let token=next;next+=1;ops.push(Op::Start(token,message(9)));ops.push(Op::Int(integer(1),*user));
                        ops.push(Op::Int(0x10e00000002,if u.instant_app{2}else if u.installed{1}else{0}));
                        ops.push(Op::Bool(boolean(3),u.hidden));ops.push(Op::Bool(boolean(4),u.suspensions.as_ref().is_some_and(|s|!s.is_empty())||!u.suspended_by.is_empty()));
                        ops.push(Op::Bool(boolean(5),u.stopped));ops.push(Op::Bool(boolean(6),!u.not_launched));ops.push(Op::Int(0x10e00000007,u.enabled));
                        if let Some(caller)=&u.last_disable_app_caller{ops.push(Op::String(string(8),caller.clone()));}
                        for suspender in &u.suspended_by{ops.push(Op::String(0x20900000009,suspender.clone()));}
                        ops.push(Op::Int(integer(10),u.distraction_flags));ops.push(Op::Long(integer(11),u.first_install_time));
                        ops.push(Op::End(token));
                    }
                    for user in self.state().users.keys(){
                        let token=next;next+=1;ops.push(Op::Start(token,message(12)));ops.push(Op::Int(integer(1),*user));
                        for permission in self.user_permission_proto(&p.name,*user)?{ops.push(Op::String(0x20900000002,permission));}
                        ops.push(Op::End(token));
                    }
                    ops.push(Op::End(package));
                }
            },
            7 => {let libraries=self.state().shared_libraries.as_ref().ok_or(NotModelled("diagnostic shared library owner absent"))?;for(i,l)in libraries.iter().enumerate(){let token=i as i32;ops.push(Op::Start(token,message(3)));if let Some(name)=&l.name{ops.push(Op::String(string(1),name.clone()));}ops.push(Op::Bool(boolean(2),l.path.is_some()));if let Some(path)=&l.path{ops.push(Op::String(string(3),path.clone()));}else if let Some(package)=&l.package_name{ops.push(Op::String(string(4),package.clone()));}ops.push(Op::End(token));}},
            _ => return Err(NotModelled("unknown native diagnostic renderer kind")),
        }
        let mut p=Parcel::new();p.write_i32(2);p.write_bool(options.title);p.write_string16(Some(&text));aim_service_aidl::write_byte_array(&mut p,fd.as_deref());p.write_i32(ops.len() as i32);
        for op in ops{match op{Op::Start(id,field)=>{p.write_i32(0);p.write_i64(field as i64);p.write_i32(id);},Op::End(id)=>{p.write_i32(1);p.write_i64(0);p.write_i32(id);},Op::Int(field,value)=>{p.write_i32(2);p.write_i64(field as i64);p.write_i32(value);},Op::Long(field,value)=>{p.write_i32(3);p.write_i64(field as i64);p.write_i64(value);},Op::String(field,value)=>{p.write_i32(5);p.write_i64(field as i64);p.write_string16(Some(&value));},Op::Bool(field,value)=>{p.write_i32(4);p.write_i64(field as i64);p.write_bool(value);}}}
        Ok(p.data().to_vec())
    }
}
