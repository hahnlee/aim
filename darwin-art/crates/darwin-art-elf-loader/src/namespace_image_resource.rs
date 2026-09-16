//! Resource keys are admitted closed-graph image names, not process-global
//! SONAME lookup or a new symbol/path permission grant.
use std::{any::Any, sync::Arc};
#[derive(Clone, Copy)]
pub(crate) enum ResourceKind {
    SourceFile,
    Provider,
}
pub(crate) struct ImageResource {
    pub name: String,
    pub kind: ResourceKind,
    pub value: Arc<dyn Any + Send + Sync>,
}
impl ImageResource {
    pub fn required_by(&self, image: &str, needed: &[String], bound: &[&str]) -> bool {
        match self.kind {
            ResourceKind::SourceFile => self.name == image,
            ResourceKind::Provider => {
                needed.contains(&self.name) || bound.contains(&self.name.as_str())
            }
        }
    }
}
