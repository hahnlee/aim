//! Native bootstrap verifier selection, PackageManagerService's two pinned selectors.
use crate::package::{apps_filter, intent::{ComponentName,Intent}, model::State, query::Query, resolve::{Resolution,Resolver}};
use aim_binder_host::parcel::{Exception,EX_ILLEGAL_STATE};
use std::sync::{Arc,Mutex};
pub struct Runtime {pub component:Option<ComponentName>,pub legacy_enabled:bool,pub diagnostics:Mutex<Vec<String>>}
/// The verifiers PackageManagerService selects: the intent filter verifier
/// (`getIntentFilterVerifierComponentNameLPr`, V1) and the domain
/// verification agent (`getDomainVerificationAgentComponentNameLPr`, V2).
pub fn verifiers(state:&State,resolution:&Resolution)->Result<(Option<ComponentName>,Option<ComponentName>),Exception> {
    let query=Query{state,filter:&resolution.apps_filter,calling_uid:1000};
    let select=|action:&str,mime:Option<&str>,permission:&str|->Result<Option<ComponentName>,Exception>{
        let intent=Intent{action:Some(action.into()),..Default::default()};
        let candidates=resolution.query_intent_receivers(&intent,mime,0x1c0000,0,1000)
            .map_err(|error|Exception::new(EX_ILLEGAL_STATE,format!("domain verifier receiver owner: {error:?}")))?;
        let mut best=None;let mut priority=i32::MIN;
        for candidate in candidates {
            let (package,class)=candidate.component();
            let Some(package_state)=state.packages.get(package) else {return Err(Exception::new(EX_ILLEGAL_STATE,"domain verifier receiver package absent"));};
            if !query.uid_has_permission(apps_filter::uid(0,package_state.app_id),permission).map_err(|error|Exception::new(EX_ILLEGAL_STATE,error.0))? {continue;}
            if best.is_none() || candidate.priority>priority {
                best=Some(ComponentName{package:package.into(),class:class.into()});priority=candidate.priority;
            }
        }
        Ok(best)
    };
    let legacy=select("android.intent.action.INTENT_FILTER_NEEDS_VERIFICATION",Some("application/vnd.android.package-archive"),"android.permission.INTENT_FILTER_VERIFICATION_AGENT")?;
    let modern=select("android.intent.action.DOMAINS_NEED_VERIFICATION",None,"android.permission.DOMAIN_VERIFICATION_AGENT")?;
    Ok((legacy,modern))
}
/// `DomainVerificationProxy.makeProxy`: the packages whose uid
/// `isCallerVerifier` accepts. A legacy verifier counts only in the
/// modern one's package.
pub fn proxy_packages(legacy:Option<ComponentName>,modern:Option<ComponentName>)->Vec<String> {
    let legacy=legacy.filter(|v1|modern.as_ref().is_none_or(|v2|v2.package==v1.package));
    modern.into_iter().chain(legacy).map(|c|c.package).collect()
}
impl Runtime {
    pub fn select(state:&Arc<State>)->Result<Arc<Self>,Exception> {
        let resolver=Resolver::default();let resolution=resolver.resolution(state).map_err(|error|Exception::new(EX_ILLEGAL_STATE,format!("domain verifier resolution: {error:?}")))?;
        let (legacy,modern)=verifiers(state,&resolution)?;
        let legacy_enabled=legacy.is_some();
        Ok(Arc::new(Self{component:modern.or(legacy),legacy_enabled,diagnostics:Mutex::new(Vec::new())}))
    }
    pub fn note(&self,message:impl Into<String>){self.diagnostics.lock().unwrap().push(message.into());}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn component(package:&str)->ComponentName {ComponentName{package:package.into(),class:"V".into()}}

    /// `makeProxy`: V2 and a V1 of its package combine; a V1 of another
    /// package is dropped, and either alone is the proxy.
    #[test]
    fn proxies_combine_as_make_proxy_combines_them() {
        assert_eq!(proxy_packages(Some(component("a")),Some(component("a"))),["a","a"]);
        assert_eq!(proxy_packages(Some(component("b")),Some(component("a"))),["a"]);
        assert_eq!(proxy_packages(Some(component("b")),None),["b"]);
        assert_eq!(proxy_packages(None,None),Vec::<String>::new());
    }
}
