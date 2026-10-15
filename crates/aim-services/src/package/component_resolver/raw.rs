//! Original queryIntentForPackage's supplied component/filter list, on the retained registry.
use super::*;
impl ComponentResolver {
    pub fn query_components(&self,kind:Kind,mut results:Results<'_>,intent:&Intent,
        ty:Option<&str>,components:&[super::super::intent::ComponentName])
        -> std::result::Result<Option<Vec<ResolveInfo>>, super::super::domain_verification::uri_parcel::MatchError> {
        if !results.state.users.contains_key(&results.user){return Ok(None);}
        let resolver=self.resolver(kind);let mut lists=Vec::new();
        for component in components {
            let mut filters=Vec::new();
            for entry in resolver.entries() {
                let Some(package)=results.state.packages.get(&entry.package).and_then(|state|state.pkg.as_ref()) else{continue;};
                if entry.package==component.package&&main_of(package,kind,entry.component).component.name==component.class {filters.push(entry);}
            }
            if !filters.is_empty(){lists.push(filters);}
        }
        let mut values=query_from_list(lists,intent,ty,results.flags & MATCH_DEFAULT_ONLY!=0,&mut results)?;
        values.sort_by(resolve_priority_order);Ok(Some(values))
    }
}
impl ComponentResolver {
    pub fn dump_registered(&self,kind:Kind,state:&State,package:Option<&str>,show_filters:bool)->String {
        use std::fmt::Write;
        let mut text=String::new();
        for entry in self.resolver(kind).entries() {
            if package.is_some_and(|package|package!=entry.package){continue;}
            let Some(code)=state.packages.get(&entry.package).and_then(|state|state.pkg.as_ref()) else{continue;};
            let component=main_of(code,kind,entry.component);
            let _=writeln!(text,"    {}/{}",entry.package,component.component.name);
            if show_filters {let _=writeln!(text,"      {:?}",entry.filter);}
        }
        text
    }
}
