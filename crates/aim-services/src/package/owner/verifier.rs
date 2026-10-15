//! Settings' live verifier owner survives an I/O failure; later writes include it.
use super::{Store, WriteError, attribute, element};
use aim_android_xml::{Element, Node, Value};
pub(super) fn replace(original: &Element, identity: Option<&str>) -> Element {
    let mut root = original.clone();
    let previous = root
        .content
        .iter()
        .position(|node| matches!(node,Node::Element(item) if item.name=="verifier"));
    root.content
        .retain(|node| !matches!(node,Node::Element(item) if item.name=="verifier"));
    if let Some(identity) = identity {
        let mut record = element("verifier");
        attribute(&mut record, "device", Some(Value::String(identity.into())));
        let index=previous.unwrap_or_else(||root.content.iter().position(|node|
            matches!(node,Node::Element(item) if item.name=="permission-trees" || item.name=="permissions")).unwrap_or(root.content.len()));
        root.content
            .insert(index.min(root.content.len()), Node::Element(record));
    }
    root
}
impl Store {
    pub fn commit_verifier_identity(
        &mut self,
        identity: crate::package::verifier::Identity,
    ) -> Result<(), WriteError> {
        let encoded = identity.encoded();
        if self
            .state
            .settings
            .verifier
            .as_ref()
            .is_some_and(|old| old != &encoded)
        {
            return Err(WriteError::before("verifier identity owner changed"));
        }
        // Original Settings installs mVerifierDeviceIdentity before writeLPr;
        // failed disk writes do not discard its live identity.
        self.state.settings.verifier = Some(encoded.clone());
        let root = replace(&self.settings_document, Some(&encoded));
        self.commit_package_document(root)
    }
}
