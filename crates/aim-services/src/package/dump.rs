//! Intrinsic Binder dump of the native package owner. Android 16 DumpUtils
//! permission rules apply before any package state is written to the owned FD.
use aim_binder_host::{local::{Call, Reply}, parcel::{Parcel, Reader, Exception, BAD_VALUE, EX_ILLEGAL_STATE}};
use super::model::State;
use std::{io::Write, sync::Arc};
use aim_binder_host::server::RetainedFd;
pub struct Dates { format: Arc<dyn Fn(i64) -> Result<String, Exception> + Send + Sync> }
impl Dates {
    pub(crate) fn new(bridge: Arc<super::bootstrap::Bridge>, system: std::sync::Weak<crate::system::System>) -> Self {
        Self { format: Arc::new(move |millis| {
            let system = system.upgrade().ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "package diagnostic system stopped"))?;
            system.check_package_bootstrap(&bridge)?;
            let value = bridge.format_package_timestamp(millis).map_err(|error| match error {
                super::bootstrap::OwnerError::Owner(error) => error,
                error => Exception::new(EX_ILLEGAL_STATE, format!("package diagnostic timestamp: {error:?}")),
            })?;
            system.check_package_bootstrap(&bridge)?;
            Ok(value)
        }) }
    }
    fn format(&self, millis: i64) -> Result<String, Exception> { (self.format)(millis) }
}
impl std::fmt::Debug for Dates {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str("Dates") }
}
impl PartialEq for Dates { fn eq(&self, other: &Self) -> bool { Arc::ptr_eq(&self.format, &other.format) } }
pub(crate) fn timestamp(out: &mut String, indent: &str, name: &str, millis: i64,
    dates: Option<&Dates>) -> Result<(), Exception> {
    use std::fmt::Write;
    let dates = dates.ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "package diagnostic date owner absent"))?;
    let _ = writeln!(out, "{indent}{name}={}", dates.format(millis)?);
    Ok(())
}
pub const DUMP_TRANSACTION: u32 = u32::from_be_bytes(*b"_DMP");
struct Request { out: RetainedFd, args: Vec<String> }
fn read_request(reader: &mut Reader<'_>, mut file: impl FnMut(u32)->Option<RetainedFd>) -> Result<Request,i32> {
    let fd=reader.read_fd()?;
    let out=file(fd).ok_or(BAD_VALUE)?;
    let args=aim_service_aidl::read_string_list(reader)?.unwrap_or_default().into_iter().map(Option::unwrap_or_default).collect();
    if reader.remaining()!=0{return Err(BAD_VALUE)}
    Ok(Request{out,args})
}
fn permission_denial(pid:i32,uid:i32,reason:&str)->String {
    format!("Permission Denial: can't dump PackageManager from from pid={pid}, uid={uid} due to {reason}\n")
}
fn allowed(
    pid:i32,uid:i32,packages:&[String],
    mut permission:impl FnMut(&str)->Result<bool,Exception>,
    mut note:impl FnMut(&str)->Result<i32,Exception>,
)->Result<Option<String>,Exception> {
    if !permission("android.permission.DUMP")? {
        return Ok(Some(permission_denial(pid,uid,"missing android.permission.DUMP permission")));
    }
    if matches!(uid,0|1000|2000|1067){return Ok(None)}
    if !permission("android.permission.PACKAGE_USAGE_STATS")? {
        return Ok(Some(permission_denial(pid,uid,"missing android.permission.PACKAGE_USAGE_STATS permission")));
    }
    for package in packages { if matches!(note(package)?,0|3){return Ok(None)} }
    Ok(Some(permission_denial(pid,uid,"android:get_usage_stats app-op not allowed")))
}

pub fn run(
    system:&Arc<crate::system::System>,call:&mut Call<'_>,
    capture:impl FnOnce()->Result<Arc<State>,Exception>,
)->Reply {
    let process=system.process();
    let mut request=read_request(&mut call.data,|fd|process.file(fd)
        .and_then(|file|aim_binder_host::server::file_fd(&file)))?;
    let result=(||{
        let uid=call.sender_euid as i32;let pid=call.sender_pid;
        if !system.check_permission("android.permission.DUMP",pid,uid)? {
            request.out.write_all(permission_denial(pid,uid,"missing android.permission.DUMP permission").as_bytes())
                .map_err(|error|Exception::new(EX_ILLEGAL_STATE,format!("package dump output: {error}")))?;
            return Ok(());
        }
        let state=capture()?;
        let user=super::apps_filter::user_id(uid);let app=super::apps_filter::app_id(uid);
        let packages=state.packages.values().filter(|package|package.app_id==app
            && super::info::user_state(package,user).installed).map(|package|package.name.clone()).collect::<Vec<_>>();
        let denial=allowed(pid,uid,&packages,|permission|if permission=="android.permission.DUMP"{Ok(true)}else{system.check_permission(permission,pid,uid)},
            |package|system.note_op_now(43,uid,package,None,""))?;
        let text=match denial { Some(text)=>text, None=>render(&state,&request.args,|names|system.capture_package_dump_permissions(&state,names))? };
        request.out.write_all(text.as_bytes()).map_err(|error|Exception::new(EX_ILLEGAL_STATE,format!("package dump output: {error}")))?;
        Ok::<_,Exception>(())
    })();
    let mut reply=Parcel::new();match result{Ok(())=>reply.write_no_exception(),Err(error)=>reply.write_exception(&error)}Ok(reply)
}

/// Settings.dumpPackageLPr SDK fields from the accepted code and setting owners.
pub(crate) fn sdk_versions(out: &mut String, setting: &super::model::PackageState) {
    use std::fmt::Write;
    let _ = write!(out, "    versionCode={}", setting.version_code);
    if let Some(code) = &setting.pkg {
        let _ = write!(out, " minSdk={}", code.min_sdk_version);
    }
    let _ = writeln!(out, " targetSdk={}", setting.target_sdk_version);
    if let Some(code) = &setting.pkg {
        out.push_str("    minExtensionVersions=[");
        if let Some(versions) = &code.min_extension_versions {
            // Original SparseIntArray.keyAt iterates SDK keys in numeric order.
            let ordered: std::collections::BTreeMap<_, _> = versions.iter().copied().collect();
            for (index, (sdk, version)) in ordered.iter().enumerate() {
                if index != 0 { out.push_str(", "); }
                let _ = write!(out, "{sdk}={version}");
            }
        }
        out.push_str("]\n");
    }
}

fn render(state:&State,args:&[String],capture_permissions:impl FnOnce(&[String])->Result<std::collections::BTreeMap<i32,super::owner::legacy_permissions::State>,Exception>)->Result<String,Exception> {
    use std::fmt::Write;
    let mut out=String::new();let mut package=None;let mut all_components=false;let mut include_apex=false;
    for arg in args {
        match arg.as_str(){
            "-a"|"-f"=>{},"--all-components"=>all_components=true,"--include-apex"=>include_apex=true,
            "-h"=>return Ok("Package manager native state dump:\n  [-a] [-f] [--all-components] [--include-apex] [packages|PACKAGE]\n".into()),
            "p"|"packages"=>{},
            value if value=="android" || (!value.starts_with('-') && value.contains('.'))=>{package=Some(value);all_components=true;},
            value=>{let _=writeln!(out,"Unsupported native package dump argument: {value}; use -h for help");return Ok(out);}
        }
    }
    if package.is_some_and(|name|!state.packages.contains_key(name)){
        let _=writeln!(out,"Unable to find package: {}",package.unwrap());return Ok(out);
    }
    let selected=state.packages.values().filter(|ps|package.is_none_or(|name|name==ps.name)
        && (include_apex || package.is_some() || ps.apex_module_name.is_none())).collect::<Vec<_>>();
    let names=selected.iter().map(|ps|ps.name.clone()).collect::<Vec<_>>();
    let permissions=capture_permissions(&names)?;
    out.push_str("Packages:\n");
    for ps in &selected {
        let _=writeln!(out,"  Package [{}]:",ps.name);
        let _=writeln!(out,"    userId={}\n    codePath={}",ps.app_id,ps.path);
        sdk_versions(&mut out, ps);
        timestamp(&mut out, "    ", "timeStamp", ps.last_modified_time, state.system.diagnostic_dates.as_deref())?;
        timestamp(&mut out, "    ", "lastUpdateTime", ps.last_update_time, state.system.diagnostic_dates.as_deref())?;
        if let Some(code)=ps.pkg.as_deref(){
            let _=writeln!(out,"    versionName={}",code.version_name.as_deref().unwrap_or("null"));
            if !code.requested_permissions.is_empty(){out.push_str("    requested permissions:\n");for permission in &code.requested_permissions{let _=writeln!(out,"      {permission}");}}
            if all_components {
                for (label,names) in [
                    ("activities",code.activities.iter().map(|value|value.main.component.name.as_str()).collect::<Vec<_>>()),
                    ("receivers",code.receivers.iter().map(|value|value.main.component.name.as_str()).collect()),
                    ("services",code.services.iter().map(|value|value.main.component.name.as_str()).collect()),
                    ("providers",code.providers.iter().map(|value|value.main.component.name.as_str()).collect()),
                ]{if !names.is_empty(){let _=writeln!(out,"    {label}:");for name in names{let _=writeln!(out,"      {name}");}}}
            }
        }
        if let Some(installer)=ps.install_source.installer.as_deref(){let _=writeln!(out,"    installerPackageName={installer}");}
        for (&id,user) in &ps.users{
            let _=writeln!(out,"    User {id}: installed={} hidden={} suspended={} stopped={} notLaunched={} enabled={} instant={} virtual={} distractionFlags={}",user.installed,user.hidden,!user.suspended_by.is_empty(),user.stopped,user.not_launched,user.enabled,user.instant_app,user.virtual_preload,user.distraction_flags);
            timestamp(&mut out, "      ", "firstInstallTime", user.first_install_time, state.system.diagnostic_dates.as_deref())?;
            if ps.shared_user_app_id.is_none() && ps.app_id>=0 {
                runtime_permissions(&mut out,"      ",permissions.get(&ps.app_id).ok_or_else(||Exception::new(EX_ILLEGAL_STATE,"package dump live permission owner absent"))?,id,package.is_some())?;
            }
            for (label, components) in [("disabledComponents", &user.disabled_components),
                ("enabledComponents", &user.enabled_components)] {
                if !components.is_empty() {
                    let _=writeln!(out,"      {label}:");
                    for component in components { let _=writeln!(out,"        {component}"); }
                }
            }
        }
    }
    let shared=selected.iter().filter_map(|ps|ps.shared_user_app_id).collect::<std::collections::BTreeSet<_>>();
    if !shared.is_empty(){
        out.push_str("Shared users:\n");
        for group in state.shared_users.values().filter(|group|shared.contains(&group.app_id)) {
            let _=writeln!(out,"  SharedUser [{}]:\n    appId={}",group.name,group.app_id);
            let live=permissions.get(&group.app_id).ok_or_else(||Exception::new(EX_ILLEGAL_STATE,"shared UID dump live permission owner absent"))?;
            for user in live.users().iter().filter(|user|!user.permissions.is_empty()) {
                let _=writeln!(out,"    User {}: ",user.id);
                runtime_permissions(&mut out,"      ",live,user.id,package.is_some())?;
            }
        }
    }
    Ok(out)
}

fn runtime_permissions(out:&mut String,prefix:&str,state:&super::owner::legacy_permissions::State,user:i32,dump_all:bool)->Result<(),Exception>{
    use std::fmt::Write;
    let user=state.user(user).ok_or_else(||Exception::new(EX_ILLEGAL_STATE,"dump permission user outside original capture"))?;
    let permissions=user.permissions.iter().filter(|permission|permission.runtime).collect::<Vec<_>>();
    if dump_all||!permissions.is_empty(){
        let _=writeln!(out,"{prefix}runtime permissions:");
        for permission in permissions {
            let name=permission.name.as_deref().unwrap_or("null");
            let _=write!(out,"{prefix}  {name}: granted={}",permission.granted);
            let mut flags=permission.flags as u32;
            if flags!=0 {
                out.push_str(", flags=[ ");
                while flags!=0 {
                    let bit=1u32<<flags.trailing_zeros();flags&=!bit;
                    out.push_str(&permission_flag(bit));if flags!=0{out.push('|');}
                }
                out.push(']');
            }
            out.push('\n');
        }
    }
    Ok(())
}
fn permission_flag(bit:u32)->String{
    let label=match bit.trailing_zeros(){
        0=>"USER_SET",1=>"USER_FIXED",2=>"POLICY_FIXED",3=>"REVOKED_COMPAT",4=>"SYSTEM_FIXED",5=>"GRANTED_BY_DEFAULT",
        6=>"REVIEW_REQUIRED",7=>"REVOKE_WHEN_REQUESTED",8=>"USER_SENSITIVE_WHEN_GRANTED",9=>"USER_SENSITIVE_WHEN_DENIED",
        11=>"RESTRICTION_INSTALLER_EXEMPT",12=>"RESTRICTION_SYSTEM_EXEMPT",13=>"RESTRICTION_UPGRADE_EXEMPT",
        14=>"APPLY_RESTRICTION",15=>"GRANTED_BY_ROLE",16=>"ONE_TIME",17=>"AUTO_REVOKED",
        _=>return (bit as i32).to_string(),
    };label.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn render_fixture(state:&State,args:&[String])->Result<String,Exception>{
        render(state,args,|names|{
            let mut output=std::collections::BTreeMap::new();
            for name in names {
                let package=&state.packages[name];let mut parcel=Parcel::new();parcel.write_i32(package.app_id);parcel.write_i32(package.users.len() as i32);
                let users=package.users.keys().copied().collect::<Vec<_>>();for user in &users {parcel.write_i32(*user);parcel.write_bool(false);parcel.write_i32(0);}
                output.insert(package.app_id,super::super::owner::legacy_permissions::State::read(parcel.data(),package.app_id,&users).unwrap());
            }Ok(output)
        })
    }
    fn live(app:i32,permissions:&[(Option<&str>,bool,bool,i32)])->super::super::owner::legacy_permissions::State{
        let mut permissions=permissions.to_vec();permissions.sort_by_key(|(name,_,_,_)|name.map_or(0,super::super::info::java_hash));
        let mut parcel=Parcel::new();parcel.write_i32(app);parcel.write_i32(1);parcel.write_i32(0);parcel.write_bool(false);parcel.write_i32(permissions.len() as i32);
        for (name,runtime,granted,flags) in permissions {parcel.write_string16(name);parcel.write_bool(runtime);parcel.write_bool(granted);parcel.write_i32(flags);}
        super::super::owner::legacy_permissions::State::read(parcel.data(),app,&[0]).unwrap()
    }
    #[test]
    fn runtime_dump_uses_live_denied_granted_and_all_original_flag_labels() {
        let mut state=State::default();state.system.diagnostic_dates=Some(Arc::new(Dates{format:Arc::new(|_|Ok("date".into()))}));
        state.packages.insert("fixture.app".into(),super::super::model::PackageState{name:"fixture.app".into(),app_id:10100,
            users:[(0,super::super::model::PackageUserState{installed:true,granted_permissions:vec!["shadow.guess".into()],..Default::default()})].into(),..Default::default()});
        let permissions=live(10100,&[(Some("permission.denied"),true,false,0),(Some("permission.granted"),true,true,(1<<11)|(1<<14)),(Some("permission.install"),false,true,0)]);
        let text=render(&state,&["fixture.app".into()],|_|Ok([(10100,permissions)].into())).unwrap();
        assert!(text.contains("      runtime permissions:\n"));
        assert!(text.contains("        permission.denied: granted=false\n"));
        assert!(text.contains("        permission.granted: granted=true, flags=[ RESTRICTION_INSTALLER_EXEMPT|APPLY_RESTRICTION]\n"));
        assert!(!text.contains("shadow.guess"));assert!(!text.contains("permission.install: granted="));
        let labels=[(0,"USER_SET"),(1,"USER_FIXED"),(2,"POLICY_FIXED"),(3,"REVOKED_COMPAT"),(4,"SYSTEM_FIXED"),(5,"GRANTED_BY_DEFAULT"),(6,"REVIEW_REQUIRED"),(7,"REVOKE_WHEN_REQUESTED"),(8,"USER_SENSITIVE_WHEN_GRANTED"),(9,"USER_SENSITIVE_WHEN_DENIED"),(11,"RESTRICTION_INSTALLER_EXEMPT"),(12,"RESTRICTION_SYSTEM_EXEMPT"),(13,"RESTRICTION_UPGRADE_EXEMPT"),(14,"APPLY_RESTRICTION"),(15,"GRANTED_BY_ROLE"),(16,"ONE_TIME"),(17,"AUTO_REVOKED")];
        for (bit,label) in labels {assert_eq!(permission_flag(1<<bit),label);}
        assert_eq!(permission_flag(1<<19),"524288");assert_eq!(permission_flag(1<<31),"-2147483648");
    }
    #[test]
    fn shared_uid_runtime_dump_is_group_scoped_without_duplicate_package_rows() {
        let mut state=State::default();state.system.diagnostic_dates=Some(Arc::new(Dates{format:Arc::new(|_|Ok("date".into()))}));
        for name in ["fixture.one","fixture.two"] {state.packages.insert(name.into(),super::super::model::PackageState{name:name.into(),app_id:0,shared_user_app_id:Some(10150),shared_user:Some("fixture.shared".into()),users:[(0,Default::default())].into(),..Default::default()});}
        state.shared_users.insert("fixture.shared".into(),super::super::model::SharedUser{name:"fixture.shared".into(),app_id:10150,packages:vec!["fixture.one".into(),"fixture.two".into()],..Default::default()});
        let permissions=live(10150,&[(Some("permission.shared"),true,false,1)]);
        let text=render(&state,&[],|_|Ok([(10150,permissions)].into())).unwrap();
        assert_eq!(text.matches("permission.shared: granted=false").count(),1);
        assert!(!text.split("Shared users:").next().unwrap().contains("runtime permissions:"));
        assert!(text.contains("SharedUser [fixture.shared]"));assert!(text.contains("    User 0: "));
    }
    #[test]
    fn dump_rejects_unowned_or_non_fd_input_before_capture() {
        let mut missing=Parcel::new();missing.write_i32(1);missing.write_i32(0);
        assert!(read_request(&mut missing.reader(),|_|panic!("non FD resolved")).is_err());
        let file=std::fs::File::open(std::env::current_exe().unwrap()).unwrap();
        let mut request=Parcel::new();request.write_file(Arc::new(file));request.write_i32(0);
        assert!(matches!(read_request(&mut request.reader(),|_|None),Err(BAD_VALUE)));
    }
    #[test]
    fn dump_requires_dump_even_for_shell_and_appops_for_other_callers() {
        let denial=allowed(81,2000,&[],|_|Ok(false),|_|panic!("shell appop")).unwrap().unwrap();
        assert!(denial.contains("pid=81, uid=2000"));assert!(denial.contains("android.permission.DUMP"));
        assert!(allowed(81,2000,&[],|name|{assert_eq!(name,"android.permission.DUMP");Ok(true)},|_|panic!("shell appop")).unwrap().is_none());
        let packages=vec!["p".into(),"q".into()];let mut notes=Vec::new();
        assert!(allowed(82,10001,&packages,|_|Ok(true),|name|{notes.push(name.to_owned());Ok(if name=="q"{3}else{1})}).unwrap().is_none());
        assert_eq!(notes,["p","q"]);
        assert!(allowed(82,10001,&packages,|_|Ok(true),|_|Ok(1)).unwrap().unwrap().contains("app-op not allowed"));
    }
    #[test]
    fn sdk_dump_preserves_setting_target_and_sparse_extension_format() {
        let mut setting = super::super::model::PackageState {
            version_code: 41,
            target_sdk_version: 35,
            ..Default::default()
        };
        let mut code = super::super::pkg::AndroidPackage {
            min_sdk_version: 23,
            target_sdk_version: 34,
            ..Default::default()
        };
        for versions in [None, Some(Vec::new()), Some(vec![(33, 5), (30, 2)])] {
            code.min_extension_versions = versions.clone();
            setting.pkg = Some(Arc::new(code.clone()));
            let mut text = String::new();
            sdk_versions(&mut text, &setting);
            let expected = if versions.as_ref().is_some_and(|values| !values.is_empty()) {
                "    versionCode=41 minSdk=23 targetSdk=35\n    minExtensionVersions=[30=2, 33=5]\n"
            } else {
                "    versionCode=41 minSdk=23 targetSdk=35\n    minExtensionVersions=[]\n"
            };
            assert_eq!(text, expected);
        }
        setting.pkg = None;
        let mut text = String::new();
        sdk_versions(&mut text, &setting);
        assert_eq!(text, "    versionCode=41 targetSdk=35\n");
    }
    #[test]
    fn component_override_sections_preserve_captured_user_set_order() {
        let mut state = State::default();
        state.system.diagnostic_dates = Some(Arc::new(Dates { format: Arc::new(|millis| Ok(millis.to_string())) }));
        state.packages.insert("p.one".into(), super::super::model::PackageState {
            name: "p.one".into(), app_id: 10001,
            users: [(0, super::super::model::PackageUserState {
                disabled_components: vec!["p.one.BB".into(), "p.one.Aa".into()],
                enabled_components: vec!["p.one.EnabledB".into(), "p.one.EnabledA".into()],
                ..Default::default()
            }), (10, Default::default())].into(), ..Default::default()
        });
        let captured = Arc::new(state.clone());
        state.packages.get_mut("p.one").unwrap().users.get_mut(&0).unwrap().disabled_components.clear();
        let expected = "      disabledComponents:\n        p.one.BB\n        p.one.Aa\n      enabledComponents:\n        p.one.EnabledB\n        p.one.EnabledA\n";
        for args in [vec![], vec!["p.one".into()]] {
            let text = render_fixture(&captured, &args).unwrap();
            assert!(text.contains(expected));
            let (_, second_user) = text.split_once("    User 10:").unwrap();
            assert!(!second_user.contains("disabledComponents:"));
            assert!(!second_user.contains("enabledComponents:"));
            assert_eq!(text.matches("disabledComponents:").count(), 1);
            assert_eq!(text.matches("enabledComponents:").count(), 1);
        }
        let latest = render_fixture(&state, &["p.one".into()]).unwrap();
        assert!(!latest.contains("disabledComponents:"));
        assert!(render_fixture(&captured, &["p.one".into()]).unwrap().contains(expected));
    }

    #[test]
    fn package_argument_reads_actual_native_package_and_user_state() {
        let mut state=State::default();
        let package=super::super::model::PackageState{name:"p.one".into(),path:"/system/app/One".into(),version_code:9,app_id:10001,
            users:[(0,super::super::model::PackageUserState{installed:true,enabled:3,first_install_time:1_234_567,..Default::default()})].into(),..Default::default()};
        state.packages.insert(package.name.clone(),package);
        state.packages.insert("p.two".into(),super::super::model::PackageState{name:"p.two".into(),..Default::default()});
        assert!(render_fixture(&state,&["p.one".into()]).unwrap_err().message.contains("date owner absent"));
        let times = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = times.clone();
        state.system.diagnostic_dates = Some(Arc::new(Dates { format: Arc::new(move |millis| {
            seen.lock().unwrap().push(millis);
            Ok(format!("guest-date-{millis}"))
        }) }));
        let text=render_fixture(&state,&["p.one".into()]).unwrap();
        assert!(text.contains("    codePath=/system/app/One\n"));
        assert!(text.contains("    User 0:"));
        assert!(text.contains("      firstInstallTime=guest-date-1234567\n"));
        assert_eq!(*times.lock().unwrap(), [0, 0, 1_234_567]);
        state.system.diagnostic_dates = Some(Arc::new(Dates { format: Arc::new(|_| {
            Err(Exception::new(EX_ILLEGAL_STATE, "guest date owner detached"))
        }) }));
        assert_eq!(render_fixture(&state,&["p.one".into()]).unwrap_err().message, "guest date owner detached");
        assert!(text.contains("Package [p.one]"));assert!(text.contains("versionCode=9"));assert!(text.contains("enabled=3"));assert!(!text.contains("p.two"));
        assert!(render_fixture(&state,&["--proto".into()]).unwrap().contains("Unsupported"));
        assert!(render_fixture(&state,&["p.absent".into()]).unwrap().contains("Unable to find package"));
    }
}
