//! Original-name public Binder fronts live for the native service process (#1076).
//! Private Computer and boot-session capabilities remain bound to their epoch.
use super::*;
use aim_service_aidl::android_os_iservicemanager as sm;
#[derive(Clone,Copy)]
pub struct Pair{pub public:Binder,pub native:Binder}
impl System{
    pub(crate) fn register_public_package_fronts(self:&Arc<Self>,bridge:&Arc<crate::package::bootstrap::Bridge>)->Result<Pair>{
        self.check_package_bootstrap(bridge)?;
        let pair={let mut slot=self.package_public_services.lock().unwrap();
            *slot.get_or_insert_with(||{let(public,native)=crate::package::service::PackageQueries::from_system(self);
                Pair{public:self.process.add_service(public),native:self.process.add_service(native)}})
        };
        for(name,binder)in[("package",pair.public),("package_native",pair.native)]{
            self.check_package_bootstrap(bridge)?;
            self.add_service(name,binder)?;
            // Query the actual registry, not a framework name cache. The native
            // owner must prove registration before returning a capability.
            let mut request=Parcel::new();sm::CheckService{name:Some(name.into())}.write(&mut request);
            let reply=self.process.transact(0,sm::CHECK_SERVICE,&request,false).map_err(|code|registration_error(format!("{name} readback transport: {code}")))?;
            let mut reader=reply.reader();let registered=sm::read_check_service_reply(&mut reader).map_err(|code|registration_error(format!("{name} readback parcel: {code}")))??;
            if reader.remaining()!=0||registered!=Some(binder){return Err(registration_error(format!("{name} registry differs from native process front: registered={registered:?}, expected={binder:?}")));}
        }
        self.check_package_bootstrap(bridge)?;Ok(pair)
    }
}
fn registration_error(message:impl Into<String>)->Exception{Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,message)}
#[cfg(test)]
#[path="system_package_public_services_tests.rs"]
mod tests;
