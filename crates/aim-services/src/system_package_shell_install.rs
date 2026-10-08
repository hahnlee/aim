//! Original install parsing policy over authenticated shell inputs.
use super::*;
use aim_service_aidl::{dev_aim_server_ipackagebootstrapbridge as bootstrap,
    dev_aim_server_ipackageshellinstallpolicy as api,ReadParcelable};
use std::os::fd::AsFd;
struct InputFile(aim_binder_driver::File);
impl aim_service_aidl::WriteParcelable for InputFile {
    fn write_to(&self,p:&mut Parcel){p.write_i32(0);p.write_file(self.0.clone());}
}
impl System {
    fn shell_install_identity(&self,uid:u32,pid:i32)->Result<i32>{
        if self.process.authenticated_inbound_identity()!=Some((pid,uid))||!matches!(uid,0|2000){
            return Err(Exception::security("install shell caller lacks authenticated provenance"));
        }
        i32::try_from(uid).map_err(|_|Exception::illegal_argument("shell UID exceeds Android range"))
    }
    fn shell_install_policy_reply(&self,code:u32,request:&Parcel)->Result<aim_binder_host::local::Received>{
        let bridge=self.package_bootstrap()?;self.check_package_bootstrap(&bridge)?;
        let owner=self.package_bootstrap_binder_leaf(&bridge,bootstrap::GET_PACKAGE_SHELL_INSTALL_POLICY)?;
        let reply=owner.transact(code,request,false).map_err(shell_install_transport)?;
        self.check_package_bootstrap(&bridge)?;Ok(reply)
    }
    pub fn shell_install_size(&self,uid:u32,pid:i32,file:std::fs::File,path:&str,abi:Option<&str>)->Result<i64>{
        let caller=self.shell_install_identity(uid,pid)?;
        let file=aim_binder_host::server::file_from_fd(file.as_fd()).ok_or_else(||shell_install_failure("install input file transfer failed"))?;
        let mut request=Parcel::new();api::CalculateInstalledSize{file:Some(InputFile(file)),path:Some(path.into()),abi:abi.map(Into::into),calling_uid:caller,calling_pid:pid}.write(&mut request);
        let reply=self.shell_install_policy_reply(api::CALCULATE_INSTALLED_SIZE,&request)?;
        let mut reader=reply.reader();let value=api::read_calculate_installed_size_reply(&mut reader).map_err(shell_install_transport)??;
        if reader.remaining()!=0{return Err(shell_install_failure("install size reply tail"));}Ok(value)
    }
    pub fn shell_validate_install_abi(&self,uid:u32,pid:i32,abi:&str)->Result<()>{
        let caller=self.shell_install_identity(uid,pid)?;let mut request=Parcel::new();
        api::ValidateAbi{abi:Some(abi.into()),calling_uid:caller,calling_pid:pid}.write(&mut request);
        let reply=self.shell_install_policy_reply(api::VALIDATE_ABI,&request)?;let mut reader=reply.reader();
        api::read_validate_abi_reply(&mut reader).map_err(shell_install_transport)??;
        if reader.remaining()!=0{return Err(shell_install_failure("install ABI reply tail"));}Ok(())
    }
    pub fn shell_validate_install_compiler_filter(&self,uid:u32,pid:i32,filter:&str)->Result<()>{
        let caller=self.shell_install_identity(uid,pid)?;let mut request=Parcel::new();
        api::ValidateCompilerFilter{filter:Some(filter.into()),calling_uid:caller,calling_pid:pid}.write(&mut request);
        let reply=self.shell_install_policy_reply(api::VALIDATE_COMPILER_FILTER,&request)?;let mut reader=reply.reader();
        api::read_validate_compiler_filter_reply(&mut reader).map_err(shell_install_transport)??;
        if reader.remaining()!=0{return Err(shell_install_failure("compiler filter reply tail"));}Ok(())
    }
    pub fn shell_dependency_installer_enabled(&self,uid:u32,pid:i32)->Result<bool>{
        let caller=self.shell_install_identity(uid,pid)?;let mut request=Parcel::new();
        api::IsDependencyInstallerEnabled{calling_uid:caller,calling_pid:pid}.write(&mut request);
        let reply=self.shell_install_policy_reply(api::IS_DEPENDENCY_INSTALLER_ENABLED,&request)?;let mut reader=reply.reader();
        let value=api::read_is_dependency_installer_enabled_reply(&mut reader).map_err(shell_install_transport)??;
        if reader.remaining()!=0{return Err(shell_install_failure("dependency installer reply tail"));}Ok(value)
    }
    pub fn shell_uninstall_apex(&self,uid:u32,pid:i32,name:&str,version:i64,user:i32,
        receiver:crate::package::installer::preapproval::IntentSender,flags:i32)->Result<()>{
        let caller=self.shell_install_identity(uid,pid)?;
        let mut parcel=Parcel::new();receiver.write_to(&mut parcel);
        let sender=crate::package::diagnostics::IntentSender::read_from(&mut parcel.reader()).map_err(shell_install_transport)?;
        self.internal_uninstall_apex(Some(name.into()),version,user,Some(sender),flags,caller,pid)
    }
}
fn shell_install_failure(message:impl Into<String>)->Exception{Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,message)}
fn shell_install_transport(status:i32)->Exception{shell_install_failure(format!("shell install policy transport: {status}"))}
