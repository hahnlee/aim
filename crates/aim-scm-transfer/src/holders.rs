use super::types::{HolderGrant, RegisteredPair};
use crate::{EndpointId, ProcessEpoch};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct HolderRecord {
    pub grant: HolderGrant,
}

impl HolderRecord {
    pub fn endpoint(self) -> EndpointId {
        self.grant.endpoint()
    }

    pub fn process(self) -> ProcessEpoch {
        self.grant.process()
    }
}

pub(crate) fn pair_for(
    endpoint_a: EndpointId,
    endpoint_b: EndpointId,
    holder_a: HolderGrant,
    holder_b: HolderGrant,
) -> RegisteredPair {
    RegisteredPair {
        endpoint_a,
        endpoint_b,
        holder_a,
        holder_b,
    }
}
