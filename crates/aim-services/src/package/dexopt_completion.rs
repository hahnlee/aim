//! Boot dexopt metrics/completion over the live native scan and original timing producer.
use aim_binder_host::{local::{Call,Reply,Service},parcel::{Parcel,Exception,UNKNOWN_TRANSACTION,EX_ILLEGAL_STATE}};
use aim_service_aidl::{dev_aim_server_ipackagedexoptcompletion as api,dev_aim_server_ipackagebootlifecycleleaf as policy};
use std::sync::{Arc,Weak};
pub struct Endpoint {
    pub system:Weak<crate::system::System>,pub bridge:Arc<super::bootstrap::Bridge>,
}
impl Endpoint {
    fn system(&self)->Result<Arc<crate::system::System>,Exception>{
        let system=self.system.upgrade().ok_or_else(||fail("Dexopt native system stopped"))?;
        system.check_package_bootstrap(&self.bridge)?;Ok(system)
    }
    fn count(&self)->Result<i32,Exception>{
        let system=self.system()?;let capture=system.capture_package_queries()?;
        let policy=system.native_dexopt_hibernation_leaf(&self.bridge)?;
        let mut count=0i32;
        for loaded in capture.scan().owner().loaded_packages().values(){
            let code=&loaded.package;
            if code.package_name=="android"||!code.is(super::pkg::booleans::HAS_CODE)||code.booleans2&super::pkg::booleans2::APEX!=0{continue;}
            let mut request=Parcel::new();policy::IsHibernationSuppressed{package_name:Some(code.package_name.clone())}.write(&mut request);
            let reply=policy.transact(policy::IS_HIBERNATION_SUPPRESSED,&request,false).map_err(|status|fail(format!("Dexopt hibernation owner: {status}")))?;
            let mut reader=reply.reader();let suppressed=policy::read_is_hibernation_suppressed_reply(&mut reader).map_err(|status|fail(format!("Dexopt hibernation reply: {status}")))??;
            if reader.remaining()!=0{return Err(fail("Dexopt hibernation trailing data"));}
            if !suppressed{count=count.checked_add(1).ok_or_else(||fail("Optimizable package count exceeds int"))?;}
        }
        self.system()?;Ok(count)
    }
}
impl Service for Endpoint {
    fn descriptor(&self)->&str{api::DESCRIPTOR}
    fn transact(&self,call:&mut Call<'_>)->Reply{
        let result=(||->Result<Parcel,Exception>{
            if call.sender_euid!=1000&&call.sender_euid!=0{return Err(Exception::security("Native dexopt completion requires system or root"));}
            let system=self.system()?;let mut reply=Parcel::new();
            match call.code {
                api::GET_BOOT_DEXOPT_START_TIME_NANOS=>{api::GetBootDexoptStartTimeNanos::read(&mut call.data).map_err(|status|fail(format!("Dexopt start request: {status}")))?;if call.data.remaining()!=0{return Err(fail("Dexopt start trailing data"));}
                    let value=system.native_boot_dexopt_start_time(&self.bridge)?;api::write_get_boot_dexopt_start_time_nanos_reply(&mut reply,value);},
                api::NOTE_BOOT_DEXOPT_START_TIME_NANOS=>{let args=api::NoteBootDexoptStartTimeNanos::read(&mut call.data).map_err(|status|fail(format!("Dexopt timing request: {status}")))?;if call.data.remaining()!=0{return Err(fail("Dexopt timing trailing data"));}
                    system.note_native_boot_dexopt_start_time(&self.bridge,args.start_time_nanos)?;api::write_note_boot_dexopt_start_time_nanos_reply(&mut reply);},
                api::GET_OPTIMIZABLE_PACKAGE_COUNT=>{api::GetOptimizablePackageCount::read(&mut call.data).map_err(|status|fail(format!("Dexopt count request: {status}")))?;if call.data.remaining()!=0{return Err(fail("Dexopt count trailing data"));}
                    api::write_get_optimizable_package_count_reply(&mut reply,self.count()?);},
                api::PERSIST_PACKAGE_USAGE=>{api::PersistPackageUsage::read(&mut call.data).map_err(|status|fail(format!("Dexopt usage request: {status}")))?;if call.data.remaining()!=0{return Err(fail("Dexopt usage trailing data"));}
                    system.persist_native_dexopt_package_usage(&self.bridge)?;api::write_persist_package_usage_reply(&mut reply);},
                _=>return Err(fail("Unknown dexopt completion transaction")),
            }
            Ok(reply)
        })();
        if !matches!(call.code,api::GET_BOOT_DEXOPT_START_TIME_NANOS|api::NOTE_BOOT_DEXOPT_START_TIME_NANOS|api::GET_OPTIMIZABLE_PACKAGE_COUNT|api::PERSIST_PACKAGE_USAGE){return Err(UNKNOWN_TRANSACTION);}
        match result{Ok(reply)=>Ok(reply),Err(error)=>{let mut reply=Parcel::new();reply.write_exception(&error);Ok(reply)}}
    }
}
fn fail(message:impl Into<String>)->Exception{Exception::new(EX_ILLEGAL_STATE,message)}
