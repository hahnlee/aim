//! Intrinsic Binder dump of the native package owner. Android 16 DumpUtils
//! permission rules apply before any package state is written to the owned FD.
use aim_binder_host::{local::{Call, Reply}, parcel::{Parcel, Reader, Exception, BAD_VALUE, EX_ILLEGAL_STATE}};
use super::model::State;
use std::{fs::File, io::Write, sync::Arc};
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
struct Request { out: File, args: Vec<String> }
fn read_request(reader: &mut Reader<'_>, mut file: impl FnMut(u32)->Option<File>) -> Result<Request,i32> {
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
        .and_then(|file|aim_binder_host::server::file_fd(&file)).map(File::from))?;
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
        let text=match denial { Some(text)=>text, None=>render(&state,&request.args)? };
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

fn render(state:&State,args:&[String])->Result<String,Exception> {
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
    out.push_str("Packages:\n");
    for ps in state.packages.values().filter(|ps|package.is_none_or(|name|name==ps.name)
        && (include_apex || package.is_some() || ps.apex_module_name.is_none())) {
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
            if !user.granted_permissions.is_empty(){out.push_str("      granted permissions:\n");for name in &user.granted_permissions{let _=writeln!(out,"        {name}: granted=true");}}
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dump_rejects_unowned_or_non_fd_input_before_capture() {
        let mut missing=Parcel::new();missing.write_i32(1);missing.write_i32(0);
        assert!(read_request(&mut missing.reader(),|_|panic!("non FD resolved")).is_err());
        let file=File::open(std::env::current_exe().unwrap()).unwrap();
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
    fn package_argument_reads_actual_native_package_and_user_state() {
        let mut state=State::default();
        let package=super::super::model::PackageState{name:"p.one".into(),path:"/system/app/One".into(),version_code:9,app_id:10001,
            users:[(0,super::super::model::PackageUserState{installed:true,enabled:3,first_install_time:1_234_567,..Default::default()})].into(),..Default::default()};
        state.packages.insert(package.name.clone(),package);
        state.packages.insert("p.two".into(),super::super::model::PackageState{name:"p.two".into(),..Default::default()});
        assert!(render(&state,&["p.one".into()]).unwrap_err().message.contains("date owner absent"));
        let times = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = times.clone();
        state.system.diagnostic_dates = Some(Arc::new(Dates { format: Arc::new(move |millis| {
            seen.lock().unwrap().push(millis);
            Ok(format!("guest-date-{millis}"))
        }) }));
        let text=render(&state,&["p.one".into()]).unwrap();
        assert!(text.contains("    codePath=/system/app/One\n"));
        assert!(text.contains("    User 0:"));
        assert!(text.contains("      firstInstallTime=guest-date-1234567\n"));
        assert_eq!(*times.lock().unwrap(), [0, 0, 1_234_567]);
        state.system.diagnostic_dates = Some(Arc::new(Dates { format: Arc::new(|_| {
            Err(Exception::new(EX_ILLEGAL_STATE, "guest date owner detached"))
        }) }));
        assert_eq!(render(&state,&["p.one".into()]).unwrap_err().message, "guest date owner detached");
        assert!(text.contains("Package [p.one]"));assert!(text.contains("versionCode=9"));assert!(text.contains("enabled=3"));assert!(!text.contains("p.two"));
        assert!(render(&state,&["--proto".into()]).unwrap().contains("Unsupported"));
        assert!(render(&state,&["p.absent".into()]).unwrap().contains("Unable to find package"));
    }
}
