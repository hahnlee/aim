//! Pinned domain backup transport: original user projection and signature hashes.
use super::owner::Owner;
use crate::package::{model::State, owner::domains};
use aim_android_xml::{Element, Node};

pub fn write(owner:&Owner,state:&State,user:i32)->Result<Vec<u8>,String> {
    let saved=owner.xml_projection();
    let empty=Element{name:"packages".into(),attrs:vec![],content:vec![]};
    let mut document=domains::replace(&empty,&saved)?;
    for section in children_mut(&mut document) {
        if section.name != "domain-verifications" {continue;}
        for collection in children_mut(section) {
            for package in children_mut(collection) {
                let name=package.string("packageName").ok_or("domain backup name absent")?.into_owned();
                if let Some(current)=state.packages.get(&name) {
                    let signing=current.signatures.as_ref().ok_or("domain backup signing owner absent")?;
                    if signing.scheme_version==0 && signing.signatures.is_empty(){return Err("domain backup UNKNOWN signing details".into());}
                    let signature=super::owner::signature_hash(&signing.signatures);
                    if let Some((_,value))=package.attrs.iter_mut().find(|(key,_)|key=="signature") {*value=aim_android_xml::Value::String(signature);}
                    else {package.attrs.push(("signature".into(),aim_android_xml::Value::String(signature)));}
                }
                if user != -1 {
                    for users in children_mut(package).filter(|child|child.name=="user-states") {
                        users.content.retain(|node|matches!(node,Node::Element(entry) if entry.int("userId").ok().flatten()==Some(user)));
                    }
                }
            }
        }
    }
    let sections=document.content.into_iter().filter_map(|node|match node{Node::Element(element)=>Some(element),_=>None}).collect::<Vec<_>>();
    aim_android_xml::abx::write_sequence(&sections)
}

fn children_mut(element:&mut Element)->impl Iterator<Item=&mut Element>{element.content.iter_mut().filter_map(|node|match node{Node::Element(child)=>Some(child),_=>None})}
