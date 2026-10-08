//! Public IPackageManager legacy domain transactions; native owner and publication.
use aim_binder_host::{local::Reply,parcel::{Exception,Parcel,Reader}};
use aim_service_aidl::android_content_pm_ipackagemanager as pm;
use std::sync::Arc;
pub fn dispatch(system:&Arc<crate::system::System>,pid:i32,uid:i32,code:u32,reader:&mut Reader<'_>)->Option<Reply> {
    if !matches!(code,pm::VERIFY_INTENT_FILTER|pm::UPDATE_INTENT_VERIFICATION_STATUS){return None;}
    Some((|| {
        let mut reply=Parcel::new();
        match code {
            pm::VERIFY_INTENT_FILTER=>{
                let args=pm::VerifyIntentFilter::read(reader)?;
                match system.verify_native_intent_filter(pid,uid,args.id,args.verification_code,
                    args.failed_domains.unwrap_or_default()) {
                    Ok(())=>pm::write_verify_intent_filter_reply(&mut reply),
                    Err(error)=>reply.write_exception(&error),
                }
            }
            pm::UPDATE_INTENT_VERIFICATION_STATUS=>{
                let args=pm::UpdateIntentVerificationStatus::read(reader)?;
                match system.update_native_intent_verification_status(pid,uid,args.package_name.as_deref(),args.user_id,args.status) {
                    Ok(value)=>pm::write_update_intent_verification_status_reply(&mut reply,value),
                    Err(error)=>reply.write_exception(&error),
                }
            }
            _=>unreachable!(),
        }
        Ok(reply)
    })())
}
/// Original verifier driver registration, before the matching v1 broadcast.
pub fn register(system:&crate::system::System,id:i32,package:String,identifier:String)->Result<(),Exception> {
    system.register_native_legacy_domain_request(id,package,identifier)
}
