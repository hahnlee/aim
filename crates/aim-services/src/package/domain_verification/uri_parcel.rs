//! Sized AIDL URI filter DTOs, android-16.0.0_r1 (AOSP, Apache-2.0).
//! Retain nullable DTO fields before applying the original group conversion.
use aim_binder_host::parcel::{BAD_VALUE, Parcel, Reader, Result};
use aim_service_aidl::{ReadParcelable, WriteParcelable};

#[derive(Debug, PartialEq, Eq)]
pub struct Filter {
    pub uri_part: i32,
    pub pattern_type: i32,
    pub filter: Option<String>,
}
#[derive(Debug, PartialEq, Eq)]
pub struct Group {
    pub action: i32,
    pub filters: Option<Vec<Option<Filter>>>,
}
#[derive(Debug, PartialEq, Eq)]
pub enum MatchError {
    NullPattern,
    InvalidPattern(String),
    IndexOutOfBounds { length: usize, index: usize },
}
impl Filter {
    /// Match DTO metadata without substituting null or suppressing constructor errors.
    pub fn match_data(
        &self,
        data: &crate::package::uri::Uri,
    ) -> std::result::Result<bool, MatchError> {
        use crate::package::intent_filter::{PATTERN_ADVANCED_GLOB, PatternMatcher};
        let matcher = self
            .filter
            .as_deref()
            .map(|pattern| PatternMatcher::new_checked(pattern, self.pattern_type))
            .transpose()
            .map_err(|error| match error {
                crate::package::intent_filter::PatternError::IllegalArgument(message) => {
                    MatchError::InvalidPattern(message)
                }
                crate::package::intent_filter::PatternError::IndexOutOfBounds { length, index } => {
                    MatchError::IndexOutOfBounds { length, index }
                }
            })?;
        if matcher.is_none() && self.pattern_type == PATTERN_ADVANCED_GLOB {
            return Err(MatchError::NullPattern);
        }
        let matches = |value: Option<&str>| {
            if value.is_none() {
                return Ok(false);
            }
            if let Some(matcher) = &matcher {
                return Ok(matcher.matches(value));
            }
            match self.pattern_type {
                0..=4 => Err(MatchError::NullPattern),
                _ => Ok(false),
            }
        };
        match self.uri_part {
            0 => matches(data.path().as_deref()),
            1 => {
                let Some(query) = data.query() else {
                    return Ok(false);
                };
                let split = |by| {
                    let mut values = query.split(by).collect::<Vec<_>>();
                    while values.len() > 1 && values.last() == Some(&"") {
                        values.pop();
                    }
                    if values.len() == 1 && values[0].is_empty() && !query.is_empty() {
                        values.clear();
                    }
                    values
                };
                let mut values = split('&');
                if values.len() == 1 {
                    values = split(';');
                }
                for value in values {
                    if matches(Some(value))? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            2 => matches(data.fragment().as_deref()),
            _ => Ok(false),
        }
    }
}
/// Original `UriRelativeFilterGroup.parcelsToGroups`, including constructor NPEs.
pub fn groups_to_model(
    parcels: Option<Vec<Option<Group>>>,
) -> std::result::Result<Vec<crate::package::intent_filter::UriRelativeFilterGroup>, &'static str> {
    let mut groups = Vec::new();
    for parcel in parcels.unwrap_or_default() {
        let parcel = parcel.ok_or("Attempt to read from field 'int android.content.UriRelativeFilterGroupParcel.action' on a null object reference in method 'void android.content.UriRelativeFilterGroup.<init>(android.content.UriRelativeFilterGroupParcel)'")?;
        let filters = parcel.filters.ok_or("Attempt to invoke interface method 'int java.util.List.size()' on a null object reference")?;
        let mut group = crate::package::intent_filter::UriRelativeFilterGroup::new(parcel.action);
        for filter in filters {
            let filter = filter.ok_or("Attempt to read from field 'int android.content.UriRelativeFilterParcel.uriPart' on a null object reference in method 'void android.content.UriRelativeFilter.<init>(android.content.UriRelativeFilterParcel)'")?;
            group.add_nullable(
                filter.uri_part,
                filter.pattern_type,
                filter.filter.as_deref(),
            );
        }
        groups.push(group);
    }
    Ok(groups)
}
fn body<'a>(reader: &mut Reader<'a>) -> Result<Reader<'a>> {
    let start = reader.position();
    let size = usize::try_from(reader.read_i32()?).map_err(|_| BAD_VALUE)?;
    let len = size.checked_sub(4).ok_or(BAD_VALUE)?;
    reader.skip(len)?;
    let (bytes, _) = reader.since(start);
    Ok(Reader::new(&bytes[4..size], &[]))
}
impl ReadParcelable for Filter {
    fn read_from(reader: &mut Reader<'_>) -> Result<Self> {
        let mut data = body(reader)?;
        let mut value = Self {
            uri_part: 0,
            pattern_type: 0,
            filter: None,
        };
        if data.remaining() != 0 {
            value.uri_part = data.read_i32()?;
        }
        if data.remaining() != 0 {
            value.pattern_type = data.read_i32()?;
        }
        if data.remaining() != 0 {
            value.filter = data.read_string16()?;
        }
        Ok(value)
    }
}
impl ReadParcelable for Group {
    fn read_from(reader: &mut Reader<'_>) -> Result<Self> {
        let mut data = body(reader)?;
        let mut value = Self {
            action: 0,
            filters: None,
        };
        if data.remaining() != 0 {
            value.action = data.read_i32()?;
        }
        if data.remaining() != 0 {
            let count = data.read_i32()?;
            if count >= 0 {
                let count = count as usize;
                if count > data.remaining() / 4 {
                    return Err(BAD_VALUE);
                }
                let mut filters = Vec::new();
                for _ in 0..count {
                    filters.push(if data.read_i32()? == 0 {
                        None
                    } else {
                        Some(Filter::read_from(&mut data)?)
                    });
                }
                value.filters = Some(filters);
            }
        }
        Ok(value)
    }
}
impl WriteParcelable for Filter {
    fn write_to(&self, out: &mut Parcel) {
        let start = out.position();
        out.write_i32(0);
        out.write_i32(self.uri_part);
        out.write_i32(self.pattern_type);
        out.write_string16(self.filter.as_deref());
        out.set_i32_at(start, (out.position() - start) as i32);
    }
}
impl WriteParcelable for Group {
    fn write_to(&self, out: &mut Parcel) {
        let start = out.position();
        out.write_i32(0);
        out.write_i32(self.action);
        match &self.filters {
            None => out.write_i32(-1),
            Some(filters) => {
                out.write_i32(filters.len() as i32);
                for filter in filters {
                    out.write_i32(i32::from(filter.is_some()));
                    if let Some(filter) = filter {
                        filter.write_to(out);
                    }
                }
            }
        }
        out.set_i32_at(start, (out.position() - start) as i32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sized_records_default_missing_fields_and_skip_future_fields() {
        for (bytes, expected) in [
            (
                vec![4],
                Filter {
                    uri_part: 0,
                    pattern_type: 0,
                    filter: None,
                },
            ),
            (
                vec![8, 2],
                Filter {
                    uri_part: 2,
                    pattern_type: 0,
                    filter: None,
                },
            ),
            (
                vec![20, 2, 1, -1, 99],
                Filter {
                    uri_part: 2,
                    pattern_type: 1,
                    filter: None,
                },
            ),
        ] {
            let mut parcel = Parcel::new();
            for word in bytes {
                parcel.write_i32(word);
            }
            let mut reader = Reader::new(parcel.data(), &[]);
            assert_eq!(Filter::read_from(&mut reader).unwrap(), expected);
            assert_eq!(reader.remaining(), 0);
        }
    }
    #[test]
    fn malformed_sizes_and_counts_fail() {
        for words in [vec![0], vec![-1], vec![100], vec![12, 0, i32::MAX]] {
            let mut parcel = Parcel::new();
            for word in words {
                parcel.write_i32(word);
            }
            assert!(Group::read_from(&mut Reader::new(parcel.data(), &[])).is_err());
        }
    }
}
