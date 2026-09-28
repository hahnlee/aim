use super::{Error, Fields, objects};

pub(super) fn validate(
    objects: &[objects::Object<'_>],
    last: Option<usize>,
    mut minimum: u64,
    parent: usize,
    offset: u64,
) -> Result<(), Error> {
    let mut cursor = last.ok_or(Error::Order)?;
    // Match original binder_validate_fixup's zero object-offset sentinel.
    if objects[cursor].offset() == 0 {
        return Err(Error::Order);
    }
    while cursor != parent {
        let Fields::Buffer {
            flags,
            parent,
            parent_offset,
            ..
        } = objects[cursor].fields()
        else {
            return Err(Error::Order);
        };
        if flags & 1 == 0 {
            return Err(Error::Order);
        }
        minimum = parent_offset.checked_add(8).ok_or(Error::Bounds)?;
        let next = usize::try_from(parent).map_err(|_| Error::Parent)?;
        if next >= cursor {
            return Err(Error::Parent);
        }
        cursor = next;
    }
    if offset < minimum {
        return Err(Error::Order);
    }
    Ok(())
}
