//! Checks that code of ours for the image links against the image's own
//! classes. Code for system_server is compiled against stubs of the
//! image's internal classes; the stubs must declare what the image
//! declares ([`ClassPath::stub_mismatches`]), and everything the code
//! refers to outside itself must resolve in the jars of the class path it
//! is loaded with, as ART resolves it: the class, then its superclasses
//! and interfaces ([`ClassPath::unresolved`]). A changed internal API thus
//! fails the build instead of the boot.

use crate::dex::{ClassDef, Dex, Value};
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// The jars of a class path.
pub struct ClassPath {
    jars: Vec<Vec<u8>>,
}

/// Immutable type relations from a class path; no class initialization occurs.
#[derive(Debug)]
pub struct Hierarchy {
    parents: HashMap<String, Vec<String>>,
}
impl Hierarchy {
    pub fn contains(&self, descriptor: &str) -> bool {
        let component = descriptor.trim_start_matches('[');
        if component != descriptor {
            return matches!(component, "B" | "C" | "D" | "F" | "I" | "J" | "S" | "Z")
                || self.parents.contains_key(component);
        }
        self.parents.contains_key(descriptor)
    }
    pub fn assignable(&self, actual: &str, required: &str) -> bool {
        if actual.starts_with('[') && self.contains(actual) {
            if matches!(
                required,
                "Ljava/lang/Object;" | "Ljava/lang/Cloneable;" | "Ljava/io/Serializable;"
            ) {
                return self.contains(required);
            }
            if let Some(component) = required.strip_prefix('[') {
                return actual == required || self.assignable(&actual[1..], component);
            }
        }
        let mut pending = vec![actual];
        let mut seen = HashSet::new();
        while let Some(next) = pending.pop() {
            if !seen.insert(next) {
                continue;
            }
            if next == required && self.contains(next) {
                return true;
            }
            if let Some(parents) = self.parents.get(next) {
                pending.extend(parents.iter().map(String::as_str));
            }
        }
        false
    }
}

/// A class's supertypes and declared members.
struct Class {
    superclass: Option<String>,
    interfaces: Vec<String>,
    fields: HashSet<(String, String)>,
    methods: HashSet<(String, String)>,
}

impl Class {
    fn read(dex: &Dex<'_>, def: &ClassDef) -> Result<Self, String> {
        let (fields, methods) = dex.members(def)?;
        Ok(Class {
            superclass: def.superclass.clone(),
            interfaces: dex.interfaces(def)?,
            fields: fields
                .into_iter()
                .map(|f| dex.field(f).map(|(_, n, t)| (n, t)))
                .collect::<Result<_, _>>()?,
            methods: methods
                .into_iter()
                .map(|m| dex.method(m).map(|(_, n, s)| (n, s)))
                .collect::<Result<_, _>>()?,
        })
    }
}

/// The classes of some dex files, read on demand; the first definition
/// of a name wins, as in a class loader.
struct Classes<'a> {
    dexes: Vec<Dex<'a>>,
    index: HashMap<String, (usize, usize)>,
    read: HashMap<String, Option<Class>>,
}

impl<'a> Classes<'a> {
    fn new(dexes: Vec<Dex<'a>>) -> Self {
        let mut index = HashMap::new();
        for (d, dex) in dexes.iter().enumerate() {
            for (c, class) in dex.classes.iter().enumerate() {
                index.entry(class.descriptor.clone()).or_insert((d, c));
            }
        }
        Self {
            dexes,
            index,
            read: HashMap::new(),
        }
    }

    fn get(&mut self, name: &str) -> Result<Option<&Class>, String> {
        if !self.read.contains_key(name) {
            let class = match self.index.get(name) {
                None => None,
                Some(&(d, c)) => Some(Class::read(&self.dexes[d], &self.dexes[d].classes[c])?),
            };
            self.read.insert(name.to_string(), class);
        }
        Ok(self.read[name].as_ref())
    }

    /// Whether `owner`, its superclasses or interfaces declare `member`.
    fn resolves(
        &mut self,
        owner: &str,
        member: &(String, String),
        method: bool,
    ) -> Result<bool, String> {
        // Constructors are resolved on their declaring class, never inherited.
        if method && member.0 == "<init>" {
            return Ok(self
                .get(owner)?
                .is_some_and(|class| class.methods.contains(member)));
        }
        let mut pending = vec![owner.to_string()];
        let mut seen = HashSet::new();
        while let Some(next) = pending.pop() {
            if !seen.insert(next.clone()) {
                continue;
            }
            let Some(class) = self.get(&next)? else {
                continue;
            };
            let members = if method {
                &class.methods
            } else {
                &class.fields
            };
            if members.contains(member) {
                return Ok(true);
            }
            pending.extend(class.superclass.iter().cloned());
            pending.extend(class.interfaces.iter().cloned());
        }
        Ok(false)
    }

    /// The static values `name` declares.
    fn static_values(&self, name: &str) -> Result<Vec<(String, Value)>, String> {
        match self.index.get(name) {
            None => Ok(Vec::new()),
            Some(&(d, c)) => self.dexes[d].static_values(&self.dexes[d].classes[c]),
        }
    }
}

impl ClassPath {
    /// The jars at `guest_paths` in the image at `root`.
    pub fn read(root: &Path, guest_paths: &[String]) -> Result<Self, String> {
        let jars = guest_paths
            .iter()
            .map(|p| {
                let host = root.join(p.trim_start_matches('/'));
                std::fs::read(&host).map_err(|e| format!("{}: {e}", host.display()))
            })
            .collect::<Result<_, _>>()?;
        Ok(Self { jars })
    }

    /// `first` (if any), then the class path's dex files.
    fn classes<'a>(&'a self, first: Option<&'a [u8]>) -> Result<Classes<'a>, String> {
        let mut dexes = Vec::new();
        if let Some(data) = first {
            dexes.push(Dex::parse(data)?);
        }
        for jar in &self.jars {
            for data in crate::system_server::dex_files(jar)? {
                dexes.push(Dex::parse(data)?);
            }
        }
        Ok(Classes::new(dexes))
    }

    pub fn hierarchy(&self) -> Result<Hierarchy, String> {
        let classes = self.classes(None)?;
        let mut parents = HashMap::new();
        for (name, (d, c)) in &classes.index {
            let dex = &classes.dexes[*d];
            let class = &dex.classes[*c];
            let mut types = dex.interfaces(class)?;
            types.extend(class.superclass.iter().cloned());
            parents.insert(name.clone(), types);
        }
        Ok(Hierarchy { parents })
    }

    /// What `dex` refers to that neither it nor the class path has, one
    /// line each; empty if everything links.
    pub fn unresolved(&self, dex: &[u8]) -> Result<Vec<String>, String> {
        let ours = Dex::parse(dex)?;
        let mut classes = self.classes(Some(dex))?;
        let mut missing = Vec::new();
        for def in &ours.classes {
            for ty in def.superclass.iter().chain(&ours.interfaces(def)?) {
                if classes.get(ty)?.is_none() {
                    missing.push(format!("{}: no class {ty}", def.descriptor));
                }
            }
        }
        let (fields, methods) = ours.ids()?;
        let mut members = Vec::new();
        for f in 0..fields {
            let (owner, name, ty) = ours.field(f)?;
            members.push((owner, (name, ty), false));
        }
        for m in 0..methods {
            let (owner, name, sig) = ours.method(m)?;
            members.push((owner, (name, sig), true));
        }
        for (owner, member, method) in members {
            // An array's members are Object's.
            let owner = if owner.starts_with('[') {
                "Ljava/lang/Object;".to_string()
            } else {
                owner
            };
            if classes.get(&owner)?.is_none() {
                missing.push(format!("no class {owner}"));
            } else if !classes.resolves(&owner, &member, method)? {
                let kind = if method { "method" } else { "field" };
                missing.push(format!("no {kind} {owner}->{}{}", member.0, member.1));
            }
        }
        missing.sort();
        missing.dedup();
        Ok(missing)
    }

    /// Where the classes of `stubs` differ from the class path's: a class,
    /// superclass, member or constant a stub declares that the image's
    /// class does not. A stub may leave members out.
    pub fn stub_mismatches(&self, stubs: &[u8]) -> Result<Vec<String>, String> {
        let stubs = Dex::parse(stubs)?;
        let mut image = self.classes(None)?;
        let mut out = Vec::new();
        for def in &stubs.classes {
            let name = &def.descriptor;
            let stub = Class::read(&stubs, def)?;
            let Some(class) = image.get(name)? else {
                out.push(format!("no class {name}"));
                continue;
            };
            if class.superclass != stub.superclass {
                out.push(format!(
                    "{name} extends {:?}, not {:?}",
                    class.superclass, stub.superclass
                ));
            }
            for (field, ty) in &stub.fields {
                if !class.fields.contains(&(field.clone(), ty.clone())) {
                    out.push(format!("no field {name}->{field}:{ty}"));
                }
            }
            // Java always emits a class constructor. A private compile-only
            // constructor may disappear from a shrunk image; executable method
            // references are still checked independently by unresolved().
            let private_constructors: HashSet<_> = stubs
                .direct_methods(def)?
                .into_iter()
                .filter(|(_, access)| access & 0x2 != 0)
                .map(|(id, _)| stubs.method(id))
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .filter(|(_, name, _)| name == "<init>")
                .map(|(_, _, sig)| sig)
                .collect();
            let constructorless = !class.methods.iter().any(|(name, _)| name == "<init>");
            for (method, sig) in &stub.methods {
                if method == "<init>" && constructorless && private_constructors.contains(sig) {
                    continue;
                }
                if method != "<clinit>" && !class.methods.contains(&(method.clone(), sig.clone())) {
                    out.push(format!("no method {name}->{method}{sig}"));
                }
            }
            let values = image.static_values(name)?;
            for (field, value) in stubs.static_values(def)? {
                if matches!(value, Value::Int(_) | Value::String(_) | Value::Bool(_))
                    && !values.contains(&(field.clone(), value.clone()))
                {
                    out.push(format!("{name}->{field} is not {value:?} in the image"));
                }
            }
        }
        out.sort();
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructors_resolve_only_on_the_requested_class() {
        let class = |parent: Option<&str>, methods: &[(&str, &str)]| Class {
            superclass: parent.map(str::to_owned),
            interfaces: Vec::new(),
            fields: HashSet::new(),
            methods: methods
                .iter()
                .map(|(n, s)| (n.to_string(), s.to_string()))
                .collect(),
        };
        let mut classes = Classes {
            dexes: Vec::new(),
            index: HashMap::new(),
            read: HashMap::from([
                (
                    "Base".into(),
                    Some(class(None, &[("<init>", "()V"), ("method", "()V")])),
                ),
                (
                    "Child".into(),
                    Some(class(Some("Base"), &[("<init>", "(I)V")])),
                ),
                ("StaticOnly".into(), Some(class(Some("Base"), &[]))),
            ]),
        };
        let member = |name: &str, sig: &str| (name.to_string(), sig.to_string());
        assert!(
            classes
                .resolves("Base", &member("<init>", "()V"), true)
                .unwrap()
        );
        assert!(
            classes
                .resolves("Child", &member("<init>", "(I)V"), true)
                .unwrap()
        );
        assert!(
            !classes
                .resolves("Child", &member("<init>", "()V"), true)
                .unwrap()
        );
        assert!(
            !classes
                .resolves("StaticOnly", &member("<init>", "()V"), true)
                .unwrap()
        );
        assert!(
            !classes
                .resolves("Missing", &member("<init>", "()V"), true)
                .unwrap()
        );
        assert!(
            classes
                .resolves("Child", &member("method", "()V"), true)
                .unwrap()
        );
    }
}
