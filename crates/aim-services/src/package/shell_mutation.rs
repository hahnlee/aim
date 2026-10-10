//! PackageManagerShellCommand mutation commands at android-16.0.0_r1.
//! Commands retain the inbound Binder caller and invoke the actual public owner.
use super::shell::Context;
use aim_binder_host::parcel::{Exception, Parcel, Reader, EX_ILLEGAL_STATE};
use aim_service_aidl::{android_content_pm_ipackagemanager as pm, android_permission_ipermissionmanager as permission};
use super::restrictions::{persistable::{Bundle, Value, Double}, dialog::DialogInfo};

fn decode<T>(value: Result<T, i32>) -> Result<T, Exception> {
    value.map_err(|status| Exception::new(EX_ILLEGAL_STATE, format!("shell reply: {status}")))
}
fn reply(reply: &Parcel) -> Result<Reader<'_>, Exception> {
    let mut reader = reply.reader();
    decode(reader.read_exception())??;
    Ok(reader)
}
fn request() -> Parcel { let mut parcel = Parcel::new(); parcel.write_interface_token(pm::DESCRIPTOR); parcel }
fn required(ctx: &mut Context<'_>) -> Result<String, Exception> {
    ctx.command.next_arg_required().map_err(Exception::illegal_argument)
}
fn user(value: &str) -> Result<i32, Exception> {
    match value { "all" => Ok(-1), "current" | "cur" => Ok(-2), _ => value.parse().map_err(|_| Exception::illegal_argument("Invalid userId")) }
}
fn error(ctx: &mut Context<'_>, message: &str) -> Result<Option<i32>, Exception> { ctx.command.eprintln(message); Ok(Some(1)) }
fn options(ctx: &mut Context<'_>, default: i32) -> Result<(i32, Vec<String>), Exception> {
    let mut user_id = default;
    let mut args = Vec::new();
    while let Some(arg) = ctx.command.next_arg() {
        if arg == "--user" && args.iter().all(|value: &String| value.starts_with("--")) { user_id = user(&required(ctx)?)?; }
        else { args.push(arg); }
    }
    Ok((user_id, args))
}
fn state_name(state: i32) -> &'static str { match state { 0=>"default",1=>"enabled",2=>"disabled",3=>"disabled-user",4=>"disabled-until-used",_=>"unknown" } }
fn permission_request() -> Parcel { let mut parcel=Parcel::new();parcel.write_interface_token(permission::DESCRIPTOR);parcel }
fn flag(name: &str) -> Option<i32> { match name { "user-set"=>Some(1),"user-fixed"=>Some(2),"revoked-compat"=>Some(8),"review-required"=>Some(64),"revoke-when-requested"=>Some(128),_=>None } }

pub fn run(ctx: &mut Context<'_>) -> Result<Option<i32>, Exception> {
    let command = ctx.command.command().unwrap_or_default().to_owned();
    match command.as_str() {
        "enable" | "disable" | "disable-user" | "disable-until-used" | "default-state" => {
            let state=match command.as_str(){"enable"=>1,"disable"=>2,"disable-user"=>3,"disable-until-used"=>4,_=>0};
            let (user_id,args)=options(ctx,0)?;
            let Some(package)=args.first() else{return error(ctx,"Error: no package or component specified")};
            let user_id=ctx.translate_user(user_id,0,"runSetEnabledSetting")?;
            let mut data=request(); let mut get=request();
            let component=super::preferred::unflatten(package);
            let (set_code,get_code,label)=if let Some(component)=component {
                for parcel in [&mut data,&mut get] { parcel.write_i32(1);parcel.write_string16(Some(&component.package));parcel.write_string16(Some(&component.class)); }
                data.write_i32(state);data.write_i32(0);data.write_i32(user_id);data.write_string16(Some("shell"));get.write_i32(user_id);
                let short=component.class.strip_prefix(&component.package).filter(|suffix|suffix.starts_with('.')).unwrap_or(&component.class);
                (pm::SET_COMPONENT_ENABLED_SETTING,pm::GET_COMPONENT_ENABLED_SETTING,format!("Component {{{}/{}}}",component.package,short))
            }else{
                data.write_string16(Some(package));data.write_i32(state);data.write_i32(0);data.write_i32(user_id);data.write_string16(Some("shell:1000"));
                get.write_string16(Some(package));get.write_i32(user_id);
                (pm::SET_APPLICATION_ENABLED_SETTING,pm::GET_APPLICATION_ENABLED_SETTING,format!("Package {package}"))
            };
            reply(&ctx.invoke_public(set_code,data)?)?;
            let result=ctx.invoke_public(get_code,get)?;let state=decode(reply(&result)?.read_i32())?;
            ctx.command.println(&format!("{label} new state: {}",state_name(state)));Ok(Some(0))
        }
        "set-install-location" => {
            let Some(value)=ctx.command.next_arg() else{return error(ctx,"Error: no install location specified.")};
            let Ok(value)=value.parse::<i32>() else{return error(ctx,"Error: install location has to be a number.")};
            let mut data=request();data.write_i32(value);
            let result=ctx.invoke_public(pm::SET_INSTALL_LOCATION,data)?;
            if decode(reply(&result)?.read_bool())?{Ok(Some(0))}else{error(ctx,"Error: install location has to be a number.")}
        }
        "set-harmful-app-warning" | "get-harmful-app-warning" => {
            let (user_id,args)=options(ctx,-2)?;
            if let Some(option)=args.first().filter(|arg|arg.starts_with('-')) {
                ctx.command.eprintln(&format!("Error: Unknown option: {option}"));
                return Ok(Some(-1));
            }
            let Some(package)=args.first() else{return Err(Exception::illegal_argument("package name required"))};
            let user_id=ctx.translate_user(user_id,0,&command)?;
            let mut data=request();data.write_string16(Some(package));
            if command=="set-harmful-app-warning" {
                data.write_i32(i32::from(args.get(1).is_some()));
                if let Some(warning)=args.get(1){data.write_i32(1);data.write_string8(Some(warning));}
                data.write_i32(user_id);reply(&ctx.invoke_public(pm::SET_HARMFUL_APP_WARNING,data)?)?;Ok(Some(0))
            }else{
                data.write_i32(user_id);let result=ctx.invoke_public(pm::GET_HARMFUL_APP_WARNING,data)?;
                let mut reader=reply(&result)?;
                let warning=if decode(reader.read_i32())?==0{None}else{decode(crate::clip::char_sequence(&mut reader))?};
                if let Some(warning)=warning.filter(|warning|!warning.is_empty()){ctx.command.println(&warning);Ok(Some(0))}else{Ok(Some(1))}
            }
        }
        "clear" => {
            let (user_id,mut args)=options(ctx,0)?;let cache=args.first().is_some_and(|value|value=="--cache-only");if cache{args.remove(0);}
            let Some(package)=args.first() else{return error(ctx,"Error: no package specified")};
            let user_id=ctx.translate_user(user_id,0,"runClear")?;
            if ctx.clear_data(package,user_id,cache)?{ctx.command.println("Success");Ok(Some(0))}else{error(ctx,"Failed")}
        }
        "reset-permissions" => {ctx.reset_runtime_permissions()?;Ok(Some(0))}
        "grant" | "revoke" => {
            let (user_id,mut args)=options(ctx,0)?;
            let all=args.first().is_some_and(|value|value=="--all-permissions");if all{args.remove(0);}
            if !all && args.is_empty(){return error(ctx,"Error: no package specified")}
            if !all && args.len()<2{return error(ctx,"Error: no permission specified")}
            if all && args.len()>1{return error(ctx,"Error: permission specified but not expected")}
            let user_id=ctx.translate_user(user_id,0,"runGrantRevokePermission")?;
            let targets=if all {ctx.requested_runtime_permissions(args.first().map(String::as_str),user_id)?}else{
                // Original shell verifies installed PackageInfo before a single grant.
                ctx.requested_runtime_permissions(Some(&args[0]),user_id)?;
                vec![(args[0].clone(),vec![args[1].clone()])]
            };
            for (package,permissions) in targets {for name in permissions {
                let mut data=permission_request();data.write_string16(Some(&package));data.write_string16(Some(&name));data.write_string16(Some("default:0"));data.write_i32(user_id);
                if command=="revoke"{data.write_string16(None);}
                let outcome=ctx.invoke_permission(if command=="grant"{permission::GRANT_RUNTIME_PERMISSION}else{permission::REVOKE_RUNTIME_PERMISSION},data).and_then(|value|reply(&value).map(|_|()));
                if let Err(failure)=outcome {if !all{return Err(failure)}ctx.command.eprintln(&format!("Could not {command} permission {name}: {failure:?}"));}
            }}Ok(Some(0))
        }
        "set-permission-flags" | "clear-permission-flags" | "get-permission-flags" => {
            let (user_id,args)=options(ctx,0)?;
            if args.is_empty(){return error(ctx,"Error: no package specified")}
            if args.len()<2{return error(ctx,"Error: no permission specified")}
            let user_id=ctx.translate_user(user_id,0,"runGrantRevokePermission")?;
            let mut data=permission_request();data.write_string16(Some(&args[0]));data.write_string16(Some(&args[1]));
            if command=="get-permission-flags" {
                data.write_string16(Some("default:0"));data.write_i32(user_id);
                let result=ctx.invoke_permission(permission::GET_PERMISSION_FLAGS,data)?;let flags=decode(reply(&result)?.read_i32())?;
                ctx.command.println(&format!("{flags}"));return Ok(Some(0));
            }
            if args.len()<3{return error(ctx,"Error: no permission flags specified")}
            let mut mask=0;for name in &args[2..]{let Some(value)=flag(name)else{return error(ctx,&format!("Error: specified flag {name} is not one of [review-required, revoked-compat, revoke-when-requested, user-fixed, user-set]"))};mask|=value;}
            data.write_i32(mask);data.write_i32(if command=="set-permission-flags"{mask}else{0});data.write_bool(true);data.write_string16(Some("default:0"));data.write_i32(user_id);
            reply(&ctx.invoke_permission(permission::UPDATE_PERMISSION_FLAGS,data)?)?;Ok(Some(0))
        }
        "suspend" | "suspend-quarantine" | "unsuspend" | "set-distracting-restriction" => suspend(ctx,&command),
        _ => Ok(None),
    }
}

fn write_bundle(parcel: &mut Parcel, bundle: Bundle) -> Result<(), Exception> {
    if bundle.entries.is_empty(){parcel.write_i32(0);}else{
        parcel.write_i32(1);
        let bytes=bundle.parcel().map_err(|error|Exception::new(EX_ILLEGAL_STATE,error))?;
        parcel.write_raw(bytes.data(),bytes.objects());
    }
    Ok(())
}
fn suspend(ctx: &mut Context<'_>, command: &str) -> Result<Option<i32>, Exception> {
    let suspended=command!="unsuspend";
    let distracting=command=="set-distracting-restriction";
    let mut user_id=0;let mut flags=0;let mut message=None;
    let mut app=Bundle::default();let mut launcher=Bundle::default();let mut packages=Vec::new();
    while let Some(arg)=ctx.command.next_arg(){
        if !packages.is_empty() || !arg.starts_with('-') {packages.push(arg);continue;}
        match arg.as_str(){
            "--user"=>user_id=user(&required(ctx)?)?,
            "--dialogMessage" if !distracting=>message=Some(required(ctx)?),
            "--flag" if distracting=>{let value=required(ctx)?;flags|=match value.as_str(){"hide-notifications"=>2,"hide-from-suggestions"=>1,_=>{ctx.command.println(&format!("Unrecognized flag: {value}"));return Ok(Some(1));}};},
            "--ael"|"--aes"|"--aed"|"--lel"|"--les"|"--led" if !distracting=>{
                let key=required(ctx)?;let text=required(ctx)?;
                if !suspended{continue;}
                let value=match arg.as_bytes()[4]{
                    b'l'=>Value::Long(text.parse().map_err(|_|Exception::illegal_argument("Invalid long extra"))?),
                    b'd'=>Value::Double(Double::new(text.parse().map_err(|_|Exception::illegal_argument("Invalid double extra"))?)),
                    _=>Value::String(text),
                };
                let bundle=if arg.starts_with("--a"){&mut app}else{&mut launcher};
                if let Some(entry)=bundle.entries.iter_mut().find(|(name,_)|name.as_deref()==Some(key.as_str())){entry.1=value;}else{bundle.entries.push((Some(key),value));}
            }
            "--"=>{while let Some(package)=ctx.command.next_arg(){packages.push(package);}break;}
            _=>{ctx.command.println(&format!("Error: Unknown option: {arg}"));return Ok(Some(1));}
        }
    }
    if packages.is_empty(){ctx.command.println("Error: package name not specified");return Ok(Some(1));}
    user_id=ctx.translate_user(user_id,0,if distracting{"set-distracting"}else{"runSuspend"})?;
    let mut data=request();data.write_i32(packages.len() as i32);for package in &packages{data.write_string16(Some(package));}
    if distracting{
        data.write_i32(flags);data.write_i32(user_id);
        let result=ctx.invoke_public(pm::SET_DISTRACTING_PACKAGE_RESTRICTIONS_AS_USER,data)?;
        let rejected=decode(aim_service_aidl::read_string_list(&mut reply(&result)?))?.unwrap_or_default();
        if !rejected.is_empty(){ctx.command.println(&format!("Could not set restriction for: [{}]",rejected.into_iter().map(|name|name.unwrap_or_else(||"null".into())).collect::<Vec<_>>().join(", ")));return Ok(Some(1));}
        return Ok(Some(0));
    }
    data.write_bool(suspended);write_bundle(&mut data,app)?;write_bundle(&mut data,launcher)?;
    if let Some(message)=message.filter(|value:&String|!value.is_empty()){
        data.write_i32(1);
        let info=DialogInfo{message:Some(message),..Default::default()};
        data.write_i32(info.icon);data.write_i32(info.title_resource);data.write_string16(info.title.as_deref());data.write_i32(info.message_resource);data.write_string16(info.message.as_deref());data.write_i32(info.button_resource);data.write_string16(info.button.as_deref());data.write_i32(info.button_action);
    }else{data.write_i32(0);}
    data.write_i32(if command=="suspend-quarantine"{1}else{0});data.write_string16(Some(if ctx.uid==0{"root"}else{"com.android.shell"}));data.write_i32(0);data.write_i32(user_id);
    reply(&ctx.invoke_public(pm::SET_PACKAGES_SUSPENDED_AS_USER,data)?)?;
    for package in packages{
        let mut data=request();data.write_string16(Some(&package));data.write_i32(user_id);
        let result=ctx.invoke_public(pm::IS_PACKAGE_SUSPENDED_FOR_USER,data)?;
        let suspended=decode(reply(&result)?.read_bool())?;
        ctx.command.println(&format!("Package {package} new suspended state: {suspended}"));
    }
    Ok(Some(0))
}
