//! Read commands from pinned PackageManagerShellCommand (AOSP Apache-2.0).
use super::{shell::Context,query::Query,apps_filter::{AppsFilter,Config},info::flags::*,resolve::Resolution};
use aim_binder_host::parcel::{Parcel,Exception,Reader,EX_ILLEGAL_STATE};
use aim_service_aidl::{WriteParcelable,android_content_pm_ipackagemanager as pm,dev_aim_server_ipackageshellreadleaf as leaf};
fn bad(message:impl Into<String>)->Exception{Exception::illegal_argument(message)}
fn decode(status:i32)->Exception{Exception::new(EX_ILLEGAL_STATE,format!("shell owner parcel: {status}"))}
fn unknown(error:super::apps_filter::NotModelled)->Exception{Exception::new(aim_binder_host::parcel::EX_UNSUPPORTED_OPERATION,error.0)}
fn user(value:&str)->Result<i32,Exception>{match value{"all"=>Ok(-1),"current"|"cur"=>Ok(-2),_=>value.parse().map_err(|_|bad(format!("Bad user number: {value}")))}}
fn incoming(ctx:&Context<'_>,user:i32,operation:&str,full:bool)->Result<i32,Exception>{ctx.system()?.handle_incoming_user(ctx.pid,ctx.uid as i32,user,full,operation,if full{Some("pm command")}else{None})}
fn required(args:&[String],at:&mut usize)->Result<String,Exception>{let value=args.get(*at).cloned().ok_or_else(||bad("Argument expected"))?;*at+=1;Ok(value)}
fn java(value:Option<&str>)->&str{value.unwrap_or("null")}
fn filter(state:&super::model::State)->Result<AppsFilter,Exception>{AppsFilter::new(state,&Config{force_system_packages_queryable:state.system.force_system_packages_queryable,force_queryable_packages:state.system.force_queryable_packages.clone()}).map_err(|error|Exception::new(EX_ILLEGAL_STATE,format!("shell query visibility: {error:?}")))}

pub fn run(ctx:&mut Context<'_>)->Result<Option<i32>,Exception>{
    let args=ctx.command.args.clone();let Some(command)=args.first().map(String::as_str)else{return Ok(None)};
    let result=match command{
        "path"=>path(ctx,&args[1..])?,
        "list"=>match args.get(1).map(String::as_str){Some("package"|"packages")=>packages(ctx,&args[2..],false)?,Some("instrumentation")=>instrumentation(ctx,&args[2..])?,Some("features")=>features(ctx)?,Some("libraries")=>libraries(ctx,&args[2..])?,Some("permissions")=>permissions(ctx,&args[2..])?,_=>return Ok(None)},
        "-l"=>packages(ctx,&args[1..],false)?,"-lf"=>packages(ctx,&args[1..],true)?,
        "resolve-activity"|"query-activities"|"query-services"|"query-receivers"=>intents(ctx,command,&args[1..])?,
        "get-install-location"=>{
            let mut request=Parcel::new();pm::GetInstallLocation{}.write(&mut request);let reply=ctx.invoke_public(pm::GET_INSTALL_LOCATION,request)?;
            let value=pm::read_get_install_location_reply(&mut reply.reader()).map_err(decode)??;
            ctx.command.println(&format!("{value}[{}]",match value{0=>"auto",1=>"internal",2=>"external",_=>"invalid"}));0
        },
        "has-feature"=>{
            let Some(name)=args.get(1)else{ctx.command.eprintln("Error: expected FEATURE name");return Ok(Some(1))};
            let version=match args.get(2){None=>0,Some(value)=>match value.parse::<i32>(){Ok(value)=>value,Err(_)=>{ctx.command.eprintln(&format!("Error: illegal version number {value}"));return Ok(Some(1))}}};
            let mut request=Parcel::new();pm::HasSystemFeature{name:Some(name.clone()),version}.write(&mut request);let reply=ctx.invoke_public(pm::HAS_SYSTEM_FEATURE,request)?;
            let found=pm::read_has_system_feature_reply(&mut reply.reader()).map_err(decode)??;ctx.command.println(if found{"true"}else{"false"});if found{0}else{1}
        },_=>return Ok(None),
    };Ok(Some(result))
}
fn instrumentation_options(args: &[String]) -> Result<(bool, Option<String>), String> {
    let mut source = false;
    let mut target = None;
    for arg in args {
        match arg.as_str() {
            "-f" => source = true,
            option if option.starts_with('-') => return Err(format!("Error: Unknown option: {option}")),
            value => target = Some(value.to_owned()),
        }
    }
    Ok((source, target))
}
fn instrumentation(ctx: &mut Context<'_>, args: &[String]) -> Result<i32, Exception> {
    let (show_source_dir, target_package) = match instrumentation_options(args) {
        Ok(value) => value,
        Err(error) => { ctx.command.println(&error); return Ok(-1); }
    };
    let mut request = Parcel::new();
    leaf::ListInstrumentation {
        show_source_dir,
        target_package,
        calling_uid: ctx.uid as i32,
        calling_pid: ctx.pid,
    }.write(&mut request);
    let reply = ctx.read_leaf(leaf::LIST_INSTRUMENTATION, request)?;
    let lines = leaf::read_list_instrumentation_reply(&mut reply.reader()).map_err(decode)??
        .ok_or_else(|| bad("instrumentation output null"))?;
    for line in lines { ctx.command.println(java(line.as_deref())); }
    Ok(0)
}

fn path(ctx:&mut Context<'_>,args:&[String])->Result<i32,Exception>{
    let mut at=0;let requested=if args.first().is_some_and(|arg|arg=="--user"){at=1;user(&required(args,&mut at)?)?}else{0};
    let name=required(args,&mut at)?;let user=incoming(ctx,requested,"runPath",true)?;
    let capture=ctx.capture()?;let filter=filter(capture.state())?;let query=Query{state:capture.state(),filter:&filter,calling_uid:ctx.uid as i32};
    let Some(info)=query.package_info(&name,-1,MATCH_APEX,user).map_err(unknown)??else{return Ok(1)};
    let Some(app)=info.application_info else{return Ok(1)};
    ctx.command.println(&format!("package:{}",java(app.source_dir.as_deref())));
    for split in app.split_source_dirs.iter().flatten(){ctx.command.println(&format!("package:{}",java(split.as_deref())));}
    Ok(0)
}
fn packages(ctx:&mut Context<'_>,args:&[String],mut source:bool)->Result<i32,Exception>{
    let(mut disabled,mut enabled,mut system,mut third,mut installer,mut uid,mut version,mut apex,mut stopped,mut quarantined)=(false,false,false,false,false,false,false,false,false,false);
    let(mut flags,mut requested,mut only_uid,mut at)=(0i64,-1,-1,0usize);
    while let Some(option)=args.get(at).filter(|value|value.starts_with('-')){at+=1;match option.as_str(){
        "-d"=>disabled=true,"-e"=>enabled=true,"-a"=>flags|=MATCH_KNOWN_PACKAGES|MATCH_HIDDEN_UNTIL_INSTALLED_COMPONENTS,"-f"=>source=true,"-i"=>installer=true,"-l"=>{},"-s"=>system=true,"-q"=>quarantined=true,"-U"=>uid=true,"-u"=>flags|=MATCH_UNINSTALLED_PACKAGES,"-3"=>third=true,"--show-versioncode"=>version=true,"--apex-only"=>{apex=true;flags|=MATCH_APEX},"--factory-only"=>flags|=MATCH_FACTORY_ONLY,"--user"=>requested=user(&required(args,&mut at)?)?,"--uid"=>{uid=true;only_uid=required(args,&mut at)?.parse().map_err(|_|bad("illegal uid"))?},"--match-libraries"=>flags|=MATCH_STATIC_SHARED_AND_SDK_LIBRARIES,"--show-stopped"=>stopped=true,_=>{ctx.command.println(&format!("Error: Unknown option: {option}"));return Ok(-1)}}
    }
    let search=args.get(at);let capture=ctx.capture()?;let filter=filter(capture.state())?;let query=Query{state:capture.state(),filter:&filter,calling_uid:ctx.uid as i32};
    let users=if requested==-1{capture.state().users.keys().copied().collect::<Vec<_>>()}else{vec![requested]};
    let mut output:Vec<(String,Vec<String>)>=Vec::new();
    for user in users{
        let user=match incoming(ctx,user,"runListPackages",true){Ok(user)=>user,Err(error)=>{ctx.command.eprintln(&format!("Error: {}",error.message));continue;}};
        for info in query.installed_packages(flags,user).map_err(unknown)??{
            let name=java(info.package_name.as_deref());if search.is_some_and(|search|!name.contains(search)){continue;}
            let app=info.application_info.as_deref();let is_system=!info.is_apex&&app.is_some_and(|app|app.flags&1!=0);let is_enabled=!info.is_apex&&app.is_some_and(|app|app.enabled);
            if only_uid!=-1&&!info.is_apex&&!app.is_some_and(|app|app.uid==only_uid){continue;}
            if (disabled&&is_enabled)||(enabled&&!is_enabled)||(system&&!is_system)||(third&&is_system)||(apex&&!info.is_apex){continue;}
            if quarantined{
                let mut request=Parcel::new();pm::IsPackageQuarantinedForUser{package_name:info.package_name.clone(),user_id:user}.write(&mut request);
                let reply=ctx.invoke_public(pm::IS_PACKAGE_QUARANTINED_FOR_USER,request)?;if !pm::read_is_package_quarantined_for_user_reply(&mut reply.reader()).map_err(decode)??{continue;}
            }
            let mut line="package:".to_owned();if source{let app=app.ok_or_else(||Exception::new(aim_binder_host::parcel::EX_NULL_POINTER,"ApplicationInfo is null"))?;line.push_str(java(app.source_dir.as_deref()));line.push('=');}line.push_str(name);
            if version{line.push_str(&format!(" versionCode:{}",app.map_or(((info.version_code_major as i64)<<32)|(info.version_code as u32 as i64),|app|app.long_version_code)));}
            if stopped{let app=app.ok_or_else(||bad("APEX has no ApplicationInfo"))?;line.push_str(&format!(" stopped={}",app.flags&0x00200000!=0));}
            if installer{let value=query.installer_package_internal(name,super::apps_filter::user_id(ctx.uid as i32)).map_err(unknown)??;line.push_str("  installer=");line.push_str(java(value.as_deref()));}
            let index=match output.iter().position(|entry|entry.0==line){Some(index)=>index,None=>{output.push((line,vec![]));output.len()-1}};
            if uid&&!info.is_apex{output[index].1.push(app.ok_or_else(||bad("missing ApplicationInfo"))?.uid.to_string());}
        }
    }
    // Java HashMap iteration: stable insertion within the final bucket layout.
    let mut capacity=16usize;while output.len()>capacity*3/4{capacity*=2;}
    output.sort_by_key(|entry|{let hash=super::info::java_hash(&entry.0) as u32;(hash^(hash>>16)) as usize&(capacity-1)});
    for(mut line,uids)in output{if !uids.is_empty(){line.push_str(" uid:");line.push_str(&uids.join(","));}ctx.command.println(&line);}Ok(0)
}
fn features(ctx:&mut Context<'_>)->Result<i32,Exception>{
    let capture=ctx.capture()?;let filter=filter(capture.state())?;let query=Query{state:capture.state(),filter:&filter,calling_uid:ctx.uid as i32};
    let mut values=query.system_available_features().items;values.sort_by(|a,b|a.name.as_ref().map(|v|v.encode_utf16().collect::<Vec<_>>()).cmp(&b.name.as_ref().map(|v|v.encode_utf16().collect::<Vec<_>>())));
    for info in values{ctx.command.println(&match info.name{None=>format!("feature:reqGlEsVersion=0x{:x}",info.req_gl_es_version),Some(name)=>if info.version>0{format!("feature:{name}={}",info.version)}else{format!("feature:{name}")}});}Ok(0)
}
fn libraries(ctx:&mut Context<'_>,args:&[String])->Result<i32,Exception>{
    let mut verbose=false;for option in args{if option=="-v"{verbose=true}else{ctx.command.println(&format!("Error: Unknown option: {option}"));return Ok(-1)}}
    let mut request=Parcel::new();pm::GetSystemSharedLibraryNamesAndPaths{}.write(&mut request);let reply=ctx.invoke_public(pm::GET_SYSTEM_SHARED_LIBRARY_NAMES_AND_PATHS,request)?;
    let mut values=pm::read_get_system_shared_library_names_and_paths_reply(&mut reply.reader()).map_err(decode)??.ok_or_else(||bad("library map null"))?;values.sort_by(|a,b|a.0.cmp(&b.0));
    for(name,path)in values{ctx.command.println(&if verbose{format!("library:{} path:{}",java(name.as_deref()),java(path.as_deref()))}else{format!("library:{}",java(name.as_deref()))});}Ok(0)
}
fn permissions(ctx:&mut Context<'_>,args:&[String])->Result<i32,Exception>{
    let(mut labels,mut groups,mut summary,mut dangerous,mut user_only,mut at)=(false,false,false,false,false,0usize);
    while let Some(option)=args.get(at).filter(|v|v.starts_with('-')){at+=1;match option.as_str(){"-d"=>dangerous=true,"-f"=>labels=true,"-g"=>groups=true,"-s"=>{groups=true;labels=true;summary=true},"-u"=>user_only=true,_=>{ctx.command.println(&format!("Error: Unknown option: {option}"));return Ok(1)}}}
    let ranges=if dangerous{let mut ranges=vec![("Dangerous Permissions:",1,1)];if user_only{ranges.push(("Normal Permissions:",0,0));}ranges}else if user_only{vec![("Dangerous and Normal Permissions:",0,1)]}else{vec![("All Permissions:",-10000,10000)]};
    for(title,min,max)in ranges{ctx.command.println(title);ctx.command.println("");let mut request=Parcel::new();leaf::ListPermissions{groups,labels,summary,minimum:min,maximum:max,group:args.get(at).cloned(),calling_uid:ctx.uid as i32,calling_pid:ctx.pid}.write(&mut request);let reply=ctx.read_leaf(leaf::LIST_PERMISSIONS,request)?;let lines=leaf::read_list_permissions_reply(&mut reply.reader()).map_err(decode)??.ok_or_else(||bad("permission output null"))?;for line in lines{ctx.command.println(java(line.as_deref()));}}Ok(0)
}
fn intents(ctx:&mut Context<'_>,command:&str,args:&[String])->Result<i32,Exception>{
    let mut request=Parcel::new();leaf::ParseIntent{arguments:Some(args.iter().cloned().map(Some).collect())}.write(&mut request);let reply=ctx.read_leaf(leaf::PARSE_INTENT,request)?;let bytes=leaf::read_parse_intent_reply(&mut reply.reader()).map_err(decode)??.ok_or_else(||bad("intent syntax record null"))?;
    let mut reader=Reader::new(&bytes,&[]);let requested=reader.read_i32().map_err(decode)?;let flags=i64::from(reader.read_i32().map_err(decode)?);let brief=reader.read_bool().map_err(decode)?;let components=reader.read_bool().map_err(decode)?;
    let intent=super::intent::Intent::read(&mut reader,&mut super::intent_filter::Plain).map_err(decode)?;if reader.remaining()!=0{return Err(bad("intent syntax tail"))}
    let user=incoming(ctx,requested,"package intent query",false)?;let capture=ctx.capture()?;let resolution=Resolution::new(capture.state().clone(),&Config{force_system_packages_queryable:capture.state().system.force_system_packages_queryable,force_queryable_packages:capture.state().system.force_queryable_packages.clone()}).map_err(|e|bad(format!("intent registry: {e:?}")))?;
    let results=match command{"resolve-activity"=>resolution.resolve_intent(&intent,intent.ty.as_deref(),flags,user,ctx.uid as i32).map(|r|r.into_iter().collect()),"query-services"=>resolution.query_intent_services(&intent,intent.ty.as_deref(),flags,user,ctx.uid as i32),"query-receivers"=>resolution.query_intent_receivers(&intent,intent.ty.as_deref(),flags,user,ctx.uid as i32),_=>resolution.query_intent_activities(&intent,intent.ty.as_deref(),flags,user,ctx.uid as i32)}.map_err(|e|Exception::new(EX_ILLEGAL_STATE,format!("native resolution: {e:?}")))?;
    let(noun,plural)=match command{"query-services"=>("Service","services"),"query-receivers"=>("Receiver","receivers"),_=>("Activity","activities")};
    if results.is_empty(){ctx.command.println(&if command=="resolve-activity"{"No activity found".into()}else{format!("No {plural} found")});return Ok(0)}
    if command!="resolve-activity"&&!components{ctx.command.println(&format!("{} {plural} found:",results.len()));}
    for(index,result)in results.iter().enumerate(){let prefix=if command=="resolve-activity"||components{""}else{ctx.command.println(&format!("  {noun} #{index}:"));"    "};
        if brief||components{if !components{ctx.command.println(&format!("{prefix}priority={} preferredOrder={} match=0x{:x} specificIndex={} isDefault={}",result.priority,result.preferred_order,result.match_,result.specific_index,result.is_default));}let(package,class)=result.component();let class=class.strip_prefix(package).filter(|rest|rest.starts_with('.')).unwrap_or(class);ctx.command.println(&format!("{prefix}{package}/{class}"));}
        else{let mut record=Parcel::new();result.write_to(&mut record);let mut request=Parcel::new();leaf::DumpResolveInfo{record:Some(record.data().to_vec()),prefix:Some(prefix.into())}.write(&mut request);let reply=ctx.read_leaf(leaf::DUMP_RESOLVE_INFO,request)?;let lines=leaf::read_dump_resolve_info_reply(&mut reply.reader()).map_err(decode)??.ok_or_else(||bad("ResolveInfo dump null"))?;for line in lines{ctx.command.println(java(line.as_deref()));}}
    }Ok(0)
}

#[cfg(test)]
mod instrumentation_tests {
    use super::instrumentation_options;
    #[test]
    fn instrumentation_options_follow_pinned_shell_source() {
        assert_eq!(instrumentation_options(&[]).unwrap(), (false, None));
        assert_eq!(instrumentation_options(&["p.first".into(), "-f".into(), "p.last".into()]).unwrap(), (true, Some("p.last".into())));
        assert_eq!(instrumentation_options(&["--user".into(), "10".into()]).unwrap_err(), "Error: Unknown option: --user");
    }
}
