//! DomainVerificationSettings.restoreSettings uses the already attached native registry.
use super::{Owner,merge_read_state,java_hash};
impl Owner {
    pub fn restore_backup(&mut self,result:crate::package::domain_verification::ReadResult,
        mut valid_domains:impl FnMut(&str)->Result<Vec<String>,String>)->Result<Vec<crate::package::domain_verification::SectionError>,String> {
        let mut incoming=result.state.restored;
        for package in result.state.active {
            crate::package::domain_verification::put(&mut incoming,package.name.clone(),package,|package|&package.name);
        }
        incoming.sort_by_key(|package|java_hash(&package.name));
        for mut package in incoming {
            let attached=self.attached.iter().position(|old|old.name==package.name);
            let pending=self.saved.active.iter().position(|old|old.name==package.name);
            let restored=self.saved.restored.iter().position(|old|old.name==package.name);
            if let Some(index)=attached {
                let valid=valid_domains(&package.name)?;merge_read_state(&mut self.attached[index],&package,&valid);
            } else if let Some(index)=pending {
                let valid=valid_domains(&package.name)?;merge_read_state(&mut self.saved.active[index],&package,&valid);
            } else if let Some(index)=restored {
                let valid=valid_domains(&package.name)?;merge_read_state(&mut self.saved.restored[index],&package,&valid);
            } else {
                package.domains.retain(|(_,state)|matches!(*state,1|5));
                for (_,state) in &mut package.domains {*state=5;}
                crate::package::domain_verification::put(&mut self.saved.restored,package.name.clone(),package,|package|&package.name);
                self.saved.restored.sort_by_key(|package|java_hash(&package.name));
            }
        }
        Ok(result.diagnostics)
    }
}
