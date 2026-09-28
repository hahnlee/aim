//! Android/Bionic ELF symbol-version matching for a selected image.
//!
//! This is deliberately separate from the strict selected-image lookup. The
//! Android linker uses the image's DT_VERSYM presence and its actual DT_VERDEF
//! index assignment, rather than requiring the requested version string to be
//! attached to an exported symbol.

use crate::{ExportedSymbol, VER_NDX_GLOBAL, VERSYM_HIDDEN};
use std::collections::HashMap;

/// Select one export according to Android's `soinfo::find_symbol_by_name`
/// version rules.
///
/// `None` for `definitions` means DT_VERSYM is absent. In that case an
/// explicit request is accepted for any matching export. `Some` means
/// DT_VERSYM is present; the requested version is looked up in the real
/// DT_VERDEF map and an unknown name deliberately uses VER_NDX_GLOBAL.
pub(super) fn selected_export(
    catalog: &[ExportedSymbol],
    definitions: Option<&HashMap<u16, String>>,
    name: &[u8],
    version: Option<&str>,
) -> Option<usize> {
    let requested_index = version.and_then(|requested| {
        definitions.map(|definitions| {
            definitions
                .iter()
                .find_map(|(&index, name)| (name == requested).then_some(index))
                .unwrap_or(VER_NDX_GLOBAL)
        })
    });

    catalog.iter().find_map(|export| {
        if export.name != name {
            return None;
        }
        let matches = match (version, definitions, requested_index) {
            // No request selects only the normal/default-visible export. This
            // also preserves the no-DT_VERSYM rule for ordinary dlsym.
            (None, _, _) => !export.version_hidden,
            // An image with no DT_VERSYM has no version index to compare.
            (Some(_), None, _) => true,
            // Explicit requests compare the masked raw index. Hidden is
            // intentionally ignored here; LOCAL0 cannot equal GLOBAL1.
            (Some(_), Some(_), Some(index)) => export
                .version_index
                .map(|raw| raw & !VERSYM_HIDDEN == index)
                .unwrap_or(false),
            (Some(_), Some(_), None) => false,
        };
        matches.then_some(export.address)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn export(
        address: usize,
        version: Option<&str>,
        hidden: bool,
        index: Option<u16>,
    ) -> ExportedSymbol {
        ExportedSymbol {
            name: b"entry".to_vec(),
            address,
            version: version.map(str::to_owned),
            version_hidden: hidden,
            version_index: index,
        }
    }

    #[test]
    fn no_versym_matches_explicit_versions_but_not_hidden_unversioned() {
        let catalog = [export(11, None, false, None), export(12, None, true, None)];
        assert_eq!(
            selected_export(&catalog, None, b"entry", Some("TEST_1")),
            Some(11)
        );
        assert_eq!(selected_export(&catalog, None, b"entry", None), Some(11));
        assert_eq!(selected_export(&catalog[1..], None, b"entry", None), None);
    }

    #[test]
    fn actual_verdef_index_matches_hidden_and_definition_without_export_name() {
        let definitions = HashMap::from([(7_u16, "TEST_1".to_owned())]);
        // The symbol's legacy `version` field is intentionally not used for
        // policy: the DT_VERDEF map is authoritative.
        let catalog = [export(21, None, true, Some(VERSYM_HIDDEN | 7))];
        assert_eq!(
            selected_export(&catalog, Some(&definitions), b"entry", Some("TEST_1")),
            Some(21)
        );
    }

    #[test]
    fn unknown_version_uses_global_but_local_does_not() {
        let definitions = HashMap::from([(7_u16, "TEST_1".to_owned())]);
        let catalog = [
            export(31, None, false, Some(0)),
            export(32, None, false, Some(VERSYM_HIDDEN | VER_NDX_GLOBAL)),
        ];
        assert_eq!(
            selected_export(&catalog, Some(&definitions), b"entry", Some("UNKNOWN")),
            Some(32)
        );
        assert_eq!(
            selected_export(&catalog[..1], Some(&definitions), b"entry", Some("UNKNOWN")),
            None
        );
    }

    #[test]
    fn no_request_still_rejects_hidden_versioned_export() {
        let definitions = HashMap::from([(7_u16, "TEST_1".to_owned())]);
        let catalog = [export(41, Some("TEST_1"), true, Some(VERSYM_HIDDEN | 7))];
        assert_eq!(
            selected_export(&catalog, Some(&definitions), b"entry", None),
            None
        );
    }
}
