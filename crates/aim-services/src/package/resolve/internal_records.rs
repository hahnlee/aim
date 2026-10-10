//! Private Computer/ResolveIntentHelper queries, android-16.0.0_r1.
use super::*;
impl Resolution {
    fn internal_activities(&self,intent:&Intent,resolved_type:Option<&str>,flags:i64,user:i32,calling_uid:i32,actual_uid:i32,resolve_for_start:bool,allow_dynamic_splits:bool)->Result<Vec<ResolveInfo>> {
        if !self.user_exists(user) {
            return Ok(Vec::new());
        }
        let flags = flags | MATCH_QUARANTINED_COMPONENTS;
        let instant_pkg = instant_app_package_name(&self.state, calling_uid)?;
        self.enforce_cross_user(actual_uid, user)?;
        let package = intent.package.as_deref();
        let (intent, original) = match (&intent.component, &intent.selector) {
            (None, Some(selector)) => (&**selector, Some(intent)),
            _ => (intent, None),
        };
        let comp = intent.component.as_ref();
        let flags = self.update_flags_for_resolve(
            flags,
            user,
            calling_uid,
            resolve_for_start,
            comp.is_some() || package.is_some(),
            self.implicit_image_capture(intent, user, resolved_type, flags)?,
        )?;
        let mut list = match comp {
            Some(comp) => {
                let mut list = Vec::new();
                let found = self
                    .query(actual_uid)
                    .activity_info(comp, flags, calling_uid, user);
                if let Some(ai) = thrown(found)?
                    && !self.block_activity(&ai, comp, flags, instant_pkg, calling_uid, user)?
                {
                    let mut ri = ResolveInfo::new(Info::Activity(ai));
                    ri.user_handle = user;
                    list.push(ri);
                    self.enforce_intent_filter_matching(
                        Kind::Activity,
                        intent,
                        resolved_type,
                        calling_uid,
                        &mut list,
                    )?;
                }
                list
            }
            None => self.query_activities_body(
                intent,
                resolved_type,
                flags,
                calling_uid,
                user,
                package,
            )?,
        };
        // blockNullAction only reports unless the block_null_action_intents
        // flag is on, which it is not on this image.
        if let Some(original) = original {
            self.enforce_intent_filter_matching(
                Kind::Activity,
                original,
                resolved_type,
                calling_uid,
                &mut list,
            )?;
        }
        self.internal_post_activities(list,instant_pkg,allow_dynamic_splits,calling_uid,resolve_for_start,user,intent)

    }
    fn internal_services(&self,intent:&Intent,resolved_type:Option<&str>,flags:i64,user:i32,calling_uid:i32,actual_uid:i32,include_instant_apps:bool,_resolve_for_start:bool)->Result<Vec<ResolveInfo>> {
        if !self.user_exists(user) {
            return Ok(Vec::new());
        }
        self.enforce_services_scope(actual_uid,user)?;
        let instant_pkg = instant_app_package_name(&self.state, calling_uid)?;
        let flags = self.update_flags_for_resolve(flags, user, calling_uid, include_instant_apps, false, false)?;
        let (intent, original) = match (&intent.component, &intent.selector) {
            (None, Some(selector)) => (&**selector, Some(intent)),
            _ => (intent, None),
        };
        let mut list = match &intent.component {
            Some(comp) => {
                let mut list = Vec::new();
                if let Some(si) = thrown(self.query(actual_uid).service_info(comp, flags, user))? {
                    let caller_instant = instant_pkg.is_some();
                    let target_instant =
                        si.info.application_info.private_flags & PRIVATE_FLAG_INSTANT != 0;
                    let hidden = si.flags & FLAG_VISIBLE_TO_INSTANT_APP == 0;
                    let block_instant = instant_pkg != Some(comp.package.as_str())
                        && ((flags & MATCH_INSTANT == 0 && !caller_instant && target_instant)
                            || (flags & MATCH_VISIBLE_TO_INSTANT_APP_ONLY != 0
                                && caller_instant
                                && hidden));
                    let block_normal = !target_instant
                        && !caller_instant
                        && should_filter_application(
                            &self.state,
                            &self.apps_filter,
                            si.info
                                .application_info
                                .item
                                .package_name
                                .as_deref()
                                .and_then(|p| self.state.packages.get(p)),
                            calling_uid,
                            user,
                            false,
                            true,
                        )?;
                    if !block_instant && !block_normal {
                        list.push(ResolveInfo::new(Info::Service(si)));
                        self.enforce_intent_filter_matching(
                            Kind::Service,
                            intent,
                            resolved_type,
                            calling_uid,
                            &mut list,
                        )?;
                    }
                }
                list
            }
            None => {
                let package = intent.package.as_deref();
                if package.is_some_and(|p| !self.has_code(p)) {
                    Vec::new()
                } else {
                    let found =
                        self.find(Kind::Service, intent, resolved_type, flags, user, package)?;
                    self.post_filter_others(
                        found.unwrap_or_default(),
                        instant_pkg,
                        calling_uid,
                        user,
                    )?
                }
            }
        };
        if let Some(original) = original {
            self.enforce_intent_filter_matching(
                Kind::Service,
                original,
                resolved_type,
                calling_uid,
                &mut list,
            )?;
        }
        Ok(list)

    }
    fn internal_receivers(&self,intent:&Intent,resolved_type:Option<&str>,flags:i64,user:i32,filter_uid:i32,for_send:bool,actual_uid:i32)->Result<Vec<ResolveInfo>> {
        let calling_uid=if for_send {SYSTEM_UID}else{filter_uid};
        if !self.user_exists(user) {
            return Ok(Vec::new());
        }
        self.enforce_cross_user(calling_uid, user)?;
        let instant_pkg = instant_app_package_name(&self.state, calling_uid)?;
        let flags = self.update_flags_for_resolve(
            flags,
            user,
            calling_uid,
            false,
            false,
            self.implicit_image_capture(intent, user, resolved_type, flags)?,
        )?;
        let (intent, original) = match (&intent.component, &intent.selector) {
            (None, Some(selector)) => (&**selector, Some(intent)),
            _ => (intent, None),
        };
        let mut list = match &intent.component {
            Some(comp) => {
                let mut list = Vec::new();
                if let Some(ai) = thrown(self.query(actual_uid).receiver_info(comp, flags, user))?
                {
                    let caller_instant = instant_pkg.is_some();
                    let target_instant =
                        ai.info.application_info.private_flags & PRIVATE_FLAG_INSTANT != 0;
                    let visible = ai.flags & FLAG_VISIBLE_TO_INSTANT_APP != 0;
                    let explicitly_visible =
                        visible && ai.flags & FLAG_IMPLICITLY_VISIBLE_TO_INSTANT_APP == 0;
                    let hidden = !visible
                        || (flags & MATCH_EXPLICITLY_VISIBLE_ONLY != 0 && !explicitly_visible);
                    let block = instant_pkg != Some(comp.package.as_str())
                        && ((flags & MATCH_INSTANT == 0 && !caller_instant && target_instant)
                            || (flags & MATCH_VISIBLE_TO_INSTANT_APP_ONLY != 0
                                && caller_instant
                                && hidden));
                    if !block {
                        list.push(ResolveInfo::new(Info::Activity(ai)));
                        self.enforce_intent_filter_matching(
                            Kind::Receiver,
                            intent,
                            resolved_type,
                            calling_uid,
                            &mut list,
                        )?;
                    }
                }
                list
            }
            None => {
                let package = intent.package.as_deref();
                let mut list = Vec::new();
                if package.is_none() {
                    list = self
                        .find(Kind::Receiver, intent, resolved_type, flags, user, None)?
                        .unwrap_or_default();
                }
                if let Some(package) = package.filter(|p| {
                    self.state
                        .packages
                        .get(*p)
                        .is_some_and(|ps| ps.pkg.is_some())
                }) {
                    list = self
                        .find(
                            Kind::Receiver,
                            intent,
                            resolved_type,
                            flags,
                            user,
                            Some(package),
                        )?
                        .unwrap_or_default();
                }
                list
            }
        };
        if let Some(original) = original {
            self.enforce_intent_filter_matching(
                Kind::Receiver,
                original,
                resolved_type,
                calling_uid,
                &mut list,
            )?;
        }
        self.apply_post_resolution_filter(list, instant_pkg, false, calling_uid, user, intent)

    }
    fn enforce_services_scope(&self,uid:i32,user:i32)->Result<()> {
        let query=self.query(uid);
        if let Err(error)=query.enforce_cross_user(user,false,false,"query intent services")? {
            let same_group=self.state.users.get(&user_id(uid)).zip(self.state.users.get(&user)).is_some_and(|(caller,target)|caller.profile_group_id>=0&&caller.profile_group_id==target.profile_group_id);
            if !same_group {return Err(ResolutionError::Original(error));}
            let policy=self.state.system.resolution_policy.as_ref().ok_or(NotModelled("actual resolution PermissionChecker owner unavailable"))?;
            let package=match self.state.uid_owners.as_ref().and_then(|owners|owners.get(&app_id(uid))) {
                Some(super::super::model::UidOwner::Package(setting))=>setting.pkg.as_ref().map(|pkg|pkg.package_name.clone()),
                Some(super::super::model::UidOwner::SharedUser(name))=>self.state.shared_users.get(name).and_then(|group|group.native_packages.as_ref()).and_then(|packages|packages.iter().find_map(|setting|setting.pkg.as_ref().map(|pkg|pkg.package_name.clone()))),
                None=>None,
            };
            if package.is_none() {return Err(ResolutionError::Original(Exception::new(aim_binder_host::parcel::EX_NULL_POINTER,"calling UID package owner is null")));}
            if policy.profile(uid,user,package).map_err(ResolutionError::Original)? {return Ok(());}
            return Err(ResolutionError::Original(Exception::security(format!("query intent services: UID {uid} requires android.permission.INTERACT_ACROSS_USERS_FULL or android.permission.INTERACT_ACROSS_USERS or android.permission.INTERACT_ACROSS_PROFILES to access user {user}."))));
        }
        Ok(())
    }
    fn internal_post_activities(&self,list:Vec<ResolveInfo>,instant:Option<&str>,dynamic:bool,uid:i32,start:bool,user:i32,intent:&Intent)->Result<Vec<ResolveInfo>> {
        let block=if intent.is_web_intent() {
            self.state.system.web_instant_policy.as_ref().ok_or(NotModelled("retained web-instant policy unavailable"))?.is_disabled(user)
        } else {false};
        let mut result=Vec::new();
        for info in list {
            if info.is_instant_app_available&&block {continue;}
            if dynamic {
                if let Info::Activity(activity)=&info.info {
                    if let Some(split)=&activity.info.split_name {
                        if !activity.info.application_info.split_names.iter().flatten().flatten().any(|name|name==split) {
                            let owner=self.state.system.instant_components.as_ref().ok_or(NotModelled("native instant installer owner unavailable"))?;
                            let Some(installer)=owner.installer_resolve_info() else {continue;};
                            if block&&activity.info.application_info.private_flags&PRIVATE_FLAG_INSTANT!=0 {continue;}
                            let package=info.component().0.to_owned();
                            let failure_intent=Intent {action:Some("android.intent.action.INSTALL_FAILURE".into()),package:Some(package.clone()),..Default::default()};
                            let failures=self.internal_activities(&failure_intent,None,0,user,uid,uid,false,false)?;
                            let failure=failures.iter().find_map(|entry|match &entry.info {Info::Activity(activity) if activity.info.split_name.is_none()=>Some(ComponentName {package:entry.component().0.into(),class:entry.component().1.into()}),_=>None});
                            let mut replacement=installer.clone();
                            replacement.auxiliary=Some(super::super::component_resolver::Auxiliary {failure,package:package.clone(),version:activity.info.application_info.long_version_code,split:split.clone()});
                            replacement.filter=Some(IntentFilter::default());replacement.resolve_package_name=Some(package);
                            replacement.label_res=if info.label_res!=0 {info.label_res}else if activity.info.item.label_res!=0 {activity.info.item.label_res}else{activity.info.application_info.item.label_res};
                            replacement.icon=if info.icon!=0 {info.icon}else if activity.info.item.icon!=0 {activity.info.item.icon}else{activity.info.application_info.item.icon};
                            replacement.is_instant_app_available=true;result.push(replacement);continue;
                        }
                    }
                }
            }
            if instant.is_none() {
                if start {result.push(info);continue;}
                let target=self.state.packages.get(info.component().0).ok_or(NotModelled("resolution target owner unavailable"))?;
                if !self.apps_filter.should_filter(&self.state,uid,target,user) {result.push(info);}
                continue;
            }
            if instant==Some(info.component().0) {result.push(info);continue;}
            if start&&(intent.is_web_intent()||intent.flags&FLAG_ACTIVITY_MATCH_EXTERNAL!=0)&&intent.package.is_none()&&intent.component.is_none() {result.push(info);continue;}
            if let Info::Activity(activity)=&info.info {if activity.flags&FLAG_VISIBLE_TO_INSTANT_APP!=0&&activity.info.application_info.private_flags&PRIVATE_FLAG_INSTANT==0 {result.push(info);}}
        }
        Ok(result)
    }
    pub fn query_activities_internal_record(&self,intent:Option<&Intent>,resolved:Option<&str>,flags:i64,private:i64,filter_uid:i32,filter_pid:i32,user:i32,start:bool,dynamic:bool,actual_uid:i32,actual_pid:i32)->std::result::Result<Vec<u8>,QueryError> {
        let result=required(intent).and_then(|intent|self.internal_activities(intent,resolved,flags,user,filter_uid,actual_uid,start,dynamic));
        let _=(private,filter_pid,actual_pid);record_list(result)
    }
    pub fn query_services_internal_record(&self,intent:Option<&Intent>,resolved:Option<&str>,flags:i64,user:i32,filter_uid:i32,filter_pid:i32,include:bool,start:bool,actual_uid:i32,actual_pid:i32)->std::result::Result<Vec<u8>,QueryError> {
        let _=(filter_pid,actual_pid);record_list(required(intent).and_then(|intent|self.internal_services(intent,resolved,flags,user,filter_uid,actual_uid,include,start)))
    }
    pub fn query_receivers_internal_record(&self,intent:Option<&Intent>,resolved:Option<&str>,flags:i64,user:i32,filter_uid:i32,filter_pid:i32,send:bool,actual_uid:i32,actual_pid:i32)->std::result::Result<Vec<u8>,QueryError> {
        let _=(filter_pid,actual_uid,actual_pid);record_list(required(intent).and_then(|intent|self.internal_receivers(intent,resolved,flags,user,filter_uid,send,actual_uid)))
    }
    pub fn resolve_service_internal_record(&self,intent:Option<&Intent>,resolved:Option<&str>,flags:i64,user:i32,filter_uid:i32,filter_pid:i32,start:bool,actual_uid:i32,actual_pid:i32)->std::result::Result<Option<Vec<u8>>,QueryError> {
        let _=(filter_pid,actual_pid);record_one(required(intent).and_then(|intent|self.internal_services(intent,resolved,flags,user,filter_uid,actual_uid,false,start).map(|mut list|(!list.is_empty()).then(||list.remove(0)))))
    }
    pub fn resolve_intent_internal_record(&self,intent:Option<&Intent>,resolved:Option<&str>,flags:i64,private:i64,user:i32,start:bool,filter_uid:i32,filter_pid:i32,actual_uid:i32,actual_pid:i32)->std::result::Result<Option<Vec<u8>>,QueryError> {
        let _=(filter_pid,actual_pid);
        record_one(required(intent).and_then(|intent| {
            if !self.user_exists(user) {return Ok(None);}
            let flags=self.update_flags_for_resolve(flags,user,filter_uid,start,false,self.implicit_image_capture(intent,user,resolved,flags)?)?;
            self.enforce_cross_user(actual_uid,user)?;
            let mut list=self.internal_activities(intent,resolved,flags,user,filter_uid,actual_uid,start,true)?;
            if start {list.retain(|info| match &info.info {Info::Activity(activity)=>activity.info.exported||activity.info.application_info.uid==filter_uid||matches!(filter_uid,0|1000),_=>false});}
            let selected=match list.len() {
                0=>None,1=>list.pop(),_=> {
                    let (first,second)=(&list[0],&list[1]);
                    if first.priority!=second.priority||first.preferred_order!=second.preferred_order||first.is_default!=second.is_default {Some(list.remove(0))}
                    else if let Some(preferred)=self.find_preferred_activity(intent,resolved,flags,&list,user,filter_uid)? {Some(preferred)}
                    else if private&2!=0 {None} else {Some(self.chooser(intent,&list,user)?)}
                }
            };
            Ok(selected.filter(|info|private&1==0||!info.handle_all_web_data_uri))
        }))
    }
}
fn required(intent:Option<&Intent>)->Result<&Intent> {intent.ok_or_else(||ResolutionError::Original(Exception::new(aim_binder_host::parcel::EX_NULL_POINTER,"intent")))}
fn write_auxiliary(parcel:&mut Parcel,list:&[ResolveInfo]) {
    parcel.write_i32(list.len() as i32);
    for info in list {
        parcel.write_bool(info.auxiliary.is_some());
        if let Some(aux)=&info.auxiliary {
            parcel.write_bool(aux.failure.is_some());if let Some(component)=&aux.failure {parcel.write_string16(Some(&component.package));parcel.write_string16(Some(&component.class));}
            parcel.write_string16(Some(&aux.package));parcel.write_i64(aux.version);parcel.write_string16(Some(&aux.split));
        }
    }
}
fn record_error(error:ResolutionError)->QueryError {
    match error {ResolutionError::NotModelled(error)=>QueryError::NotModelled(error),ResolutionError::Original(error)=>QueryError::Original(error),ResolutionError::UriMatching(error)=>match error.binder_exception(){Some(error)=>QueryError::Original(error),None=>QueryError::Transport(aim_binder_host::parcel::UNKNOWN_TRANSACTION)}}
}
fn record_list(result:Result<Vec<ResolveInfo>>)->std::result::Result<Vec<u8>,QueryError> {
    let list=result.map_err(record_error)?;let mut parcel=Parcel::new();parcel.write_i32(list.len() as i32);
    for info in &list {aim_service_aidl::write_typed(&mut parcel,Some(info));}
    write_auxiliary(&mut parcel,&list);Ok(parcel.data().to_vec())
}
fn record_one(result:Result<Option<ResolveInfo>>)->std::result::Result<Option<Vec<u8>>,QueryError> {
    let Some(info)=result.map_err(record_error)? else {return Ok(None);};let mut parcel=Parcel::new();
    aim_service_aidl::WriteParcelable::write_to(&info,&mut parcel);write_auxiliary(&mut parcel,&[info]);Ok(Some(parcel.data().to_vec()))
}
