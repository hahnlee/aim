//! Original UM inventories for Settings constructor persistence side effects.
use super::*;
use aim_service_aidl::{dev_aim_server_ipackagebootstrapbridge as bootstrap,
    dev_aim_server_ipackagebootconfigurationleaf as leaf};

pub(crate) struct BootPersistenceUsers {
    pub all: Vec<i32>,
    pub active: Vec<i32>,
    leaf: Strong,
}
impl BootPersistenceUsers {
    pub(crate) fn revalidate(&self, system: &System, bridge: &Arc<crate::package::bootstrap::Bridge>) -> Result<()> {
        system.check_package_bootstrap(bridge)?;
        let (all,active)=read_users(&self.leaf)?;
        if all!=self.all||active!=self.active {return Err(boot_write_error("original persistence user inventories changed"));}
        system.check_package_bootstrap(bridge)?;
        Ok(())
    }
}
impl System {
    pub(crate) fn native_boot_persistence_users(&self, bridge: &Arc<crate::package::bootstrap::Bridge>) -> Result<BootPersistenceUsers> {
        self.check_package_bootstrap(bridge)?;
        let factory = self.boot_configuration_for(bridge)?.values.lock().unwrap().policy.as_ref()
            .map(|(policy,_)|policy.factory_test).ok_or_else(||boot_write_error("original boot invocation unavailable"))?;
        let node = self.package_bootstrap_binder_leaf_with(bridge,bootstrap::GET_PACKAGE_BOOT_CONFIGURATION_LEAF,|parcel|parcel.write_bool(factory))?;
        let (all,active)=read_users(&node)?;
        self.check_package_bootstrap(bridge)?;
        Ok(BootPersistenceUsers{all,active,leaf:node})
    }
}
fn read_users(node: &Strong) -> Result<(Vec<i32>, Vec<i32>)> {
    let mut request=Parcel::new();request.write_interface_token(leaf::DESCRIPTOR);
    leaf::GetRuntimePermissionUserIds{}.write(&mut request);
    let reply=node.transact(leaf::GET_RUNTIME_PERMISSION_USER_IDS,&request,false).map_err(|status|boot_write_error(format!("original runtime-user inventory: {status}")))?;
    let mut reader=reply.reader();
    let all=leaf::read_get_runtime_permission_user_ids_reply(&mut reader).map_err(|status|boot_write_error(format!("runtime-user inventory reply: {status}")))??
        .ok_or_else(||boot_write_error("original runtime-user inventory null"))?;
    if reader.remaining()!=0{return Err(boot_write_error("runtime-user inventory reply tail"));}
    let mut request=Parcel::new();request.write_interface_token(leaf::DESCRIPTOR);
    leaf::GetPackageListActiveUserIds{}.write(&mut request);
    let reply=node.transact(leaf::GET_PACKAGE_LIST_ACTIVE_USER_IDS,&request,false).map_err(|status|boot_write_error(format!("original active-user inventory: {status}")))?;
    let mut reader=reply.reader();
    let active=leaf::read_get_package_list_active_user_ids_reply(&mut reader).map_err(|status|boot_write_error(format!("active-user inventory reply: {status}")))??
        .ok_or_else(||boot_write_error("original active-user inventory null"))?;
    if reader.remaining()!=0{return Err(boot_write_error("active-user inventory reply tail"));}
    let all_set=all.iter().copied().collect::<std::collections::BTreeSet<_>>();
    let active_set=active.iter().copied().collect::<std::collections::BTreeSet<_>>();
    if all.iter().any(|id|*id<0)||active.iter().any(|id|*id<0)||all_set.len()!=all.len()||active_set.len()!=active.len()||!active_set.is_subset(&all_set){
        return Err(boot_write_error("original boot user inventories disagree"));
    }
    Ok((all,active))
}

fn boot_write_error(message:impl Into<String>)->Exception{Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,message)}
