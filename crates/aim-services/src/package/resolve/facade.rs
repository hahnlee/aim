//! Computed original Computer reads, separate from public resolution guards.
use super::*;
impl Resolution {
    pub fn cross_profile_domain_approval(&self,intent:Option<&Intent>,ty:Option<&str>,flags:i64,
        source:i32,parent:i32)->Result<Option<i32>> {
        let intent=intent.ok_or_else(||ResolutionError::Original(Exception::new(aim_binder_host::parcel::EX_NULL_POINTER,"cross-profile intent is null")))?;
        let users=self.state.system.user_policy.as_ref().ok_or(NotModelled("actual parent app-linking owner unavailable"))?;
        if !users.parent_app_linking(source).map_err(ResolutionError::Original)?{return Ok(None);}
        let candidates=self.find(Kind::Activity,intent,ty,flags,parent,None)?;
        let Some(candidates)=candidates else{return Ok(None);};
        let mut level=None;
        for candidate in candidates {
            if candidate.handle_all_web_data_uri{continue;}
            let Some(package)=self.state.packages.get(candidate.component().0) else{continue;};
            let approved=if let Some(host)=intent.data.as_ref().and_then(|data|data.host()) {
                let groups=package.uri_relative_filter_groups.iter().find(|(domain,_)|domain==&host).map_or(&[][..],|(_,groups)|groups.as_slice());
                if !groups.is_empty()&&!super::super::intent_filter::UriRelativeFilterGroup::match_groups(groups,intent.data.as_ref().unwrap()).map_err(ResolutionError::UriMatching)? {0}
                else{self.approval_level(package,&host,parent)?}
            }else{0};
            level=Some(level.map_or(approved,|old:i32|old.max(approved)));
        }
        Ok(level.filter(|level|*level>0))
    }
}
