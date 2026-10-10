//! Captured ComponentResolver API: raw registered filters, without public Computer policy.
use super::query_state::Capture;
use crate::package::{apps_filter::NotModelled,component_resolver::{Kind,Results},intent::{Intent,ComponentName},info};
use aim_binder_host::parcel::Parcel;
use aim_service_aidl::WriteParcelable;
pub(crate) enum RawError { Transport(i32), Original(aim_binder_host::parcel::Exception), Gap(NotModelled) }
impl Capture {
    pub fn raw_components_record(&self,resolution:&crate::package::resolve::Resolution,kind:i32,
        intent:Option<&Intent>,ty:Option<&str>,flags:i64,package:Option<&str>,subset:Option<&[Option<ComponentName>]>,user:i32)
        ->Result<Vec<u8>,RawError> {

        let kind=match kind {0=>Kind::Activity,1=>Kind::Receiver,2=>Kind::Service,3=>Kind::Provider,_=>return Err(RawError::Transport(aim_binder_host::parcel::BAD_VALUE))};
        let intent=intent.ok_or_else(||RawError::Original(aim_binder_host::parcel::Exception::new(aim_binder_host::parcel::EX_NULL_POINTER,"intent is null")))?;
        let results=Results{state:self.state(),user,flags};
        let values=match subset {
            None=>resolution.components.query(kind,results,intent,ty,package),
            Some(subset)=>{
                let subset=subset.iter().map(|component|component.clone().ok_or(RawError::Transport(aim_binder_host::parcel::BAD_VALUE))).collect::<Result<Vec<_>,_>>()?;
                resolution.components.query_components(kind,results,intent,ty,&subset)
            },
        }.map_err(|error|match error.binder_exception(){Some(exception)=>RawError::Original(exception),None=>RawError::Transport(aim_binder_host::parcel::UNKNOWN_TRANSACTION)})?;
        let mut parcel=Parcel::new();
        match values {None=>parcel.write_i32(-1),Some(values)=>{parcel.write_i32(values.len() as i32);for value in values{parcel.write_i32(1);value.write_to(&mut parcel);}}}
        Ok(parcel.data().to_vec())
    }
    pub fn raw_provider(&self,authority:Option<&str>,flags:i64,user:i32)->Result<Option<info::ProviderInfo>,NotModelled> {
        let Some(authority)=authority else{return Ok(None);};
        if !self.state().users.contains_key(&user){return Ok(None);}
        let registry=self.state().package_registry.as_ref().ok_or(NotModelled("native provider authority registry unavailable"))?;
        let Some(provider)=registry.authority(authority) else{return Ok(None);};
        let Some(package)=self.state().packages.get(&provider.package) else{return Ok(None);};
        let Some(code)=package.pkg.as_deref() else{return Ok(None);};
        let state=package.users.get(&user).cloned().unwrap_or_default();
        let target=info::Target{sys:&self.state().system,pkg:code,ps:package,state:&state,user};
        let Some(application)=info::generate_application_info(&target,flags) else{return Ok(None);};
        Ok(info::generate_provider_info(&target,&provider.value,flags,Some(std::sync::Arc::new(application))))
    }
    pub fn raw_providers_record(&self,process:Option<&str>,metadata:Option<&str>,uid:i32,flags:i64,user:i32)->Result<Vec<u8>,RawError> {
        let mut parcel=Parcel::new();
        if !self.state().users.contains_key(&user){parcel.write_i32(-1);return Ok(parcel.data().to_vec());}
        let registry=self.state().package_registry.as_ref().ok_or(RawError::Gap(NotModelled("raw provider registration owner unavailable")))?;
        let mut entries=registry.providers();entries.sort_by_key(|entry|crate::package::info::java_hash(&entry.package).wrapping_add(crate::package::info::java_hash(&entry.value.main.component.name)));
        let mut values=Vec::new();
        for entry in entries.into_iter().rev() {
            if entry.value.authority.is_none(){continue;}
            let Some(package)=self.state().packages.get(&entry.package) else{continue;};let Some(code)=package.pkg.as_deref() else{continue;};
            if process.is_some_and(|process|entry.value.main.process_name.as_deref()!=Some(process)||crate::package::apps_filter::app_id(code.uid)!=crate::package::apps_filter::app_id(uid)){continue;}
            if let Some(key)=metadata {
                let data=entry.value.main.component.meta_data.as_ref().ok_or_else(||RawError::Original(aim_binder_host::parcel::Exception::new(aim_binder_host::parcel::EX_NULL_POINTER,"provider metadata is null")))?;
                if !data.0.iter().any(|(name,_)|name==key){continue;}
            }
            let state=package.users.get(&user).cloned().unwrap_or_default();let target=info::Target{sys:&self.state().system,pkg:code,ps:package,state:&state,user};
            let Some(application)=info::generate_application_info(&target,flags) else{continue;};
            if let Some(provider)=info::generate_provider_info(&target,&entry.value,flags,Some(std::sync::Arc::new(application))){values.push(provider);}
        }
        if values.is_empty(){parcel.write_i32(-1);}else{parcel.write_i32(values.len() as i32);for value in values{parcel.write_i32(1);value.write_to(&mut parcel);}}
        Ok(parcel.data().to_vec())
    }
    pub fn raw_sync_providers_record(&self,safe:bool,user:i32)->Result<Vec<u8>,NotModelled> {
        let registry=self.state().package_registry.as_ref().ok_or(NotModelled("raw provider authority owner unavailable"))?;
        let mut values=Vec::new();
        for (name,entry) in registry.ordered_authorities().into_iter().rev() {
            if !entry.value.syncable{continue;}
            let Some(package)=self.state().packages.get(&entry.package) else{continue;};let Some(code)=package.pkg.as_deref() else{continue;};
            if safe&&!package.is.system{continue;}
            let state=package.users.get(&user).cloned().unwrap_or_default();let target=info::Target{sys:&self.state().system,pkg:code,ps:package,state:&state,user};
            let Some(application)=info::generate_application_info(&target,0) else{continue;};
            if let Some(provider)=info::generate_provider_info(&target,&entry.value,0,Some(std::sync::Arc::new(application))){values.push((name,provider));}
        }
        let mut parcel=Parcel::new();parcel.write_i32(values.len() as i32);
        for (name,value) in values{parcel.write_string16(Some(name));parcel.write_i32(1);value.write_to(&mut parcel);}
        Ok(parcel.data().to_vec())
    }

    pub fn raw_components_dump_record(&self,resolution:&crate::package::resolve::Resolution,kind:i32,package:Option<&str>,options:Option<&[u8]>)->Result<Vec<u8>,NotModelled> {
        use std::fmt::Write;
        let (mut title,show)=if let Some(options)=options {
            let mut reader=aim_binder_host::parcel::Reader::new(options,&[]);
            if reader.read_i32().map_err(|_|NotModelled("raw dump frame"))?!=1{return Err(NotModelled("raw dump version"));}
            let _=reader.read_i32().map_err(|_|NotModelled("raw dump types"))?;
            let flags=reader.read_i32().map_err(|_|NotModelled("raw dump options"))?;
            (reader.read_bool().map_err(|_|NotModelled("raw dump title"))?,flags&1!=0)
        }else{(false,false)};
        let mut text=String::new();
        if (0..=3).contains(&kind) {
            let value=match kind{0=>Kind::Activity,1=>Kind::Receiver,2=>Kind::Service,_=>Kind::Provider};
            let contents=resolution.components.dump_registered(value,self.state(),package,show);
            if !contents.is_empty(){if title{text.push('\n');}let _=writeln!(text,"{} Resolver Table:",match kind{0=>"Activity",1=>"Receiver",2=>"Service",_=>"Provider"});text.push_str(&contents);title=true;}
        }else if kind==4 {
            let registry=self.state().package_registry.as_ref().ok_or(NotModelled("content provider registry unavailable"))?;
            let mut printed=false;
            for provider in registry.providers() {
                if package.is_some_and(|name|name!=provider.package){continue;}
                if !printed{if title{text.push('\n');}text.push_str("Registered ContentProviders:\n");printed=true;title=true;}
                let _=writeln!(text,"  {}/{} authority={:?}",provider.package,provider.value.main.component.name,provider.value.authority);
            }
            if printed {text.push_str("\nContentProvider Authorities:\n");for(authority,provider)in registry.ordered_authorities(){if package.is_some_and(|name|name!=provider.package){continue;}let _=writeln!(text,"  [{authority}]:\n    {}/{}",provider.package,provider.value.main.component.name);}}
        }else if kind==5 {
            if title{text.push('\n');}text.push_str("Service permissions:\n");title=true;
            for package in self.state().packages.values(){if let Some(code)=&package.pkg{for service in &code.services{if let Some(permission)=&service.permission{for _ in &service.main.component.intents{let _=writeln!(text,"    {}/{}: {permission}",package.name,service.main.component.name);}}}}}
        }else{return Err(NotModelled("raw resolver dump kind"));}
        let mut parcel=Parcel::new();parcel.write_bool(title);parcel.write_string16(Some(&text));Ok(parcel.data().to_vec())
    }

}
