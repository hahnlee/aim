//! Legacy resolver migration into the actual per-user document owner.
use super::Store;
use crate::package::settings::native_read::NativeRead;
use aim_android_xml::Node;
impl Store {
    pub fn seed_native_readers(&mut self,seed:&NativeRead)->Result<(),String>{
        let Some(root)=self.restrictions.get_mut(&0) else{return Ok(());};
        let preferred=&seed.preferred_user_zero;
        for (name,present,document) in [
            ("preferred-activities",preferred.preferred_resolver_present,preferred.preferred_document(&(0..preferred.preferred.entries().len()).collect::<Vec<_>>(),true)?),
            ("persistent-preferred-activities",preferred.persistent_resolver_present,preferred.persistent_document(&(0..preferred.persistent.entries().len()).collect::<Vec<_>>())?),
            ("crossProfile-intent-filters",preferred.cross_profile_resolver_present,preferred.cross_profile_document(&(0..preferred.cross_profile.entries().len()).collect::<Vec<_>>())?),
        ]{
            // User restrictions are read after packages.xml. Their own section
            // clears/replaces the corresponding resolver, including empty ones.
            if present&&!root.children().any(|child|child.name==name){root.content.push(Node::Element(document));}
        }
        if crate::package::preferred::has_preferred_resolver(root){self.preferred_users.insert(0);}
        Ok(())
    }
}
