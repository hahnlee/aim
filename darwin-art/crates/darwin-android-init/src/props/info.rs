//! `property_info`: the serialized trie that maps property names to SELinux
//! contexts and types (system/core/property_service/libpropertyinfoserializer
//! and libpropertyinfoparser).
//!
//! [`build_trie`] reproduces `BuildTrie` byte for byte, including the arena
//! allocation order, 4-byte alignment and zero padding, the sorted string
//! tables, and libc++'s `std::sort` ordering of equal-length prefixes.
//! [`PropertyInfoArea`] is the reader bionic and the property service use.

use std::collections::BTreeSet;

use crate::libbase::{is_c_space, split, trim};

/// Default context and type init passes to `BuildTrie`.
pub const DEFAULT_CONTEXT: &str = "u:object_r:default_prop:s0";
pub const DEFAULT_TYPE: &str = "string";

/// One line of a `*_property_contexts` file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PropertyInfoEntry {
    pub name: String,
    pub context: String,
    /// Space-joined type tokens (`string`, `bool`, `enum a b c`, ...), or
    /// empty for older files without types.
    pub type_: String,
    pub exact_match: bool,
}

impl PropertyInfoEntry {
    pub fn new(name: &str, context: &str, type_: &str, exact_match: bool) -> Self {
        Self {
            name: name.to_string(),
            context: context.to_string(),
            type_: type_.to_string(),
            exact_match,
        }
    }
}

/// `SpaceTokenizer::GetNext`.
fn next_token<'a>(rest: &mut &'a str) -> &'a str {
    let bytes = rest.as_bytes();
    let end = bytes
        .iter()
        .position(|b| is_c_space(*b))
        .unwrap_or(bytes.len());
    let token = &rest[..end];
    let skip = bytes[end..].iter().take_while(|b| is_c_space(**b)).count();
    *rest = &rest[end + skip..];
    token
}

fn is_type_valid(type_strings: &[&str]) -> bool {
    match type_strings.first() {
        None => false,
        Some(&"enum") => type_strings.len() > 1,
        Some(first) => {
            type_strings.len() == 1
                && ["string", "bool", "int", "uint", "double", "size"].contains(first)
        }
    }
}

fn parse_property_info_line(
    line: &str,
    require_prefix_or_exact: bool,
) -> Result<PropertyInfoEntry, String> {
    let mut rest = line;
    let property = next_token(&mut rest);
    if property.is_empty() {
        return Err(format!("Did not find a property entry in '{line}'"));
    }
    let context = next_token(&mut rest);
    if context.is_empty() {
        return Err(format!("Did not find a context entry in '{line}'"));
    }
    let match_operation = next_token(&mut rest);
    let mut type_strings = Vec::new();
    loop {
        let token = next_token(&mut rest);
        if token.is_empty() {
            break;
        }
        type_strings.push(token);
    }
    let exact_match = match_operation == "exact";
    if !exact_match
        && match_operation != "prefix"
        && !match_operation.is_empty()
        && require_prefix_or_exact
    {
        return Err(format!(
            "Match operation '{match_operation}' is not valid: must be either 'prefix' or 'exact'"
        ));
    }
    if !type_strings.is_empty() && !is_type_valid(&type_strings) {
        return Err(format!("Type '{}' is not valid", type_strings.join(" ")));
    }
    Ok(PropertyInfoEntry {
        name: property.to_string(),
        context: context.to_string(),
        type_: type_strings.join(" "),
        exact_match,
    })
}

/// `ParsePropertyInfoFile`: appends the entries of one contexts file and
/// returns the per-line errors (which init only logs).
pub fn parse_property_info_file(
    contents: &str,
    require_prefix_or_exact: bool,
    entries: &mut Vec<PropertyInfoEntry>,
) -> Vec<String> {
    let mut errors = Vec::new();
    for line in split(contents, '\n') {
        let trimmed = trim(&line);
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        match parse_property_info_line(trimmed, require_prefix_or_exact) {
            Ok(entry) => entries.push(entry),
            Err(error) => errors.push(error),
        }
    }
    errors
}

/// The `*_property_contexts` files `CreateSerializedPropertyInfo` reads, in
/// order, when `/system/etc/selinux/plat_property_contexts` exists.
pub const PROPERTY_CONTEXTS_FILES: &[&str] = &[
    "/system/etc/selinux/plat_property_contexts",
    "/system_ext/etc/selinux/system_ext_property_contexts",
    "/vendor/etc/selinux/vendor_property_contexts",
    "/product/etc/selinux/product_property_contexts",
    "/odm/etc/selinux/odm_property_contexts",
];

/// The ramdisk fallback set used when the system file is absent.
pub const PROPERTY_CONTEXTS_FILES_RAMDISK: &[&str] = &[
    "/plat_property_contexts",
    "/system_ext_property_contexts",
    "/vendor_property_contexts",
    "/product_property_contexts",
    "/odm_property_contexts",
];

// ---------------------------------------------------------------------------
// TrieBuilder

#[derive(Clone, Debug)]
struct EntryBuilder {
    name: String,
    context: Option<String>,
    type_: Option<String>,
}

#[derive(Clone, Debug)]
struct BuilderNode {
    entry: EntryBuilder,
    children: Vec<BuilderNode>,
    prefixes: Vec<EntryBuilder>,
    exact_matches: Vec<EntryBuilder>,
}

impl BuilderNode {
    fn new(name: &str) -> Self {
        Self {
            entry: EntryBuilder {
                name: name.to_string(),
                context: None,
                type_: None,
            },
            children: Vec::new(),
            prefixes: Vec::new(),
            exact_matches: Vec::new(),
        }
    }

    fn child_index_or_add(&mut self, name: &str) -> usize {
        if let Some(index) = self.children.iter().position(|c| c.entry.name == name) {
            return index;
        }
        self.children.push(BuilderNode::new(name));
        self.children.len() - 1
    }
}

struct TrieBuilder {
    root: BuilderNode,
    contexts: BTreeSet<String>,
    types: BTreeSet<String>,
}

impl TrieBuilder {
    fn new(default_context: &str, default_type: &str) -> Self {
        let mut root = BuilderNode::new("root");
        root.entry.context = Some(default_context.to_string());
        root.entry.type_ = Some(default_type.to_string());
        Self {
            root,
            contexts: BTreeSet::from([default_context.to_string()]),
            types: BTreeSet::from([default_type.to_string()]),
        }
    }

    fn add(&mut self, entry: &PropertyInfoEntry) -> Result<(), String> {
        // StringPointerFromContainer inserts even when the entry then fails.
        self.contexts.insert(entry.context.clone());
        self.types.insert(entry.type_.clone());
        let context = Some(entry.context.clone());
        let type_ = Some(entry.type_.clone());

        let mut pieces = split(&entry.name, '.');
        let mut ends_with_dot = false;
        if pieces.last().is_some_and(String::is_empty) {
            ends_with_dot = true;
            pieces.pop();
        }
        // Split never returns an empty vector, but "." leaves nothing after
        // popping; upstream then reads front() of an empty vector (UB).
        if pieces.is_empty() {
            return Err(format!("Unable to add '{}'", entry.name));
        }
        let mut node = &mut self.root;
        while pieces.len() > 1 {
            let piece = pieces.remove(0);
            let index = node.child_index_or_add(&piece);
            node = &mut node.children[index];
        }
        let last = pieces.remove(0);
        if entry.exact_match {
            if node.exact_matches.iter().any(|e| e.name == last) {
                return Err(format!(
                    "Duplicate exact match detected for '{}'",
                    entry.name
                ));
            }
            node.exact_matches.push(EntryBuilder {
                name: last,
                context,
                type_,
            });
        } else if !ends_with_dot {
            if node.prefixes.iter().any(|e| e.name == last) {
                return Err(format!(
                    "Duplicate prefix match detected for '{}'",
                    entry.name
                ));
            }
            node.prefixes.push(EntryBuilder {
                name: last,
                context,
                type_,
            });
        } else {
            let index = node.child_index_or_add(&last);
            let child = &mut node.children[index];
            if child.entry.context.is_some() || child.entry.type_.is_some() {
                return Err(format!(
                    "Duplicate prefix match detected for '{}'",
                    entry.name
                ));
            }
            child.entry.context = context;
            child.entry.type_ = type_;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// TrieSerializer

struct Arena {
    data: Vec<u8>,
}

impl Arena {
    fn allocate(&mut self, size: usize) -> u32 {
        let aligned = (size + 3) & !3;
        let offset = self.data.len();
        self.data.resize(offset + aligned, 0);
        offset as u32
    }

    fn write_u32(&mut self, offset: u32, value: u32) {
        let offset = offset as usize;
        self.data[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
    }

    fn allocate_string(&mut self, s: &str) -> u32 {
        let offset = self.allocate(s.len() + 1);
        let start = offset as usize;
        self.data[start..start + s.len()].copy_from_slice(s.as_bytes());
        offset
    }

    fn size(&self) -> u32 {
        self.data.len() as u32
    }
}

const HEADER_SIZE: usize = 24;
const PROPERTY_ENTRY_SIZE: usize = 16;
const TRIE_NODE_SIZE: usize = 28;

struct Serializer<'a> {
    arena: Arena,
    contexts: &'a BTreeSet<String>,
    types: &'a BTreeSet<String>,
}

impl Serializer<'_> {
    fn serialize_strings(&mut self, strings: &BTreeSet<String>) {
        let count_offset = self.arena.allocate(4);
        self.arena.write_u32(count_offset, strings.len() as u32);
        let array = self.arena.allocate(4 * strings.len());
        for (index, string) in strings.iter().enumerate() {
            let offset = self.arena.allocate_string(string);
            self.arena.write_u32(array + 4 * index as u32, offset);
        }
    }

    fn index_of(set: &BTreeSet<String>, value: &Option<String>) -> u32 {
        match value {
            Some(value) if !value.is_empty() => set
                .iter()
                .position(|s| s == value)
                .map_or(u32::MAX, |index| index as u32),
            _ => u32::MAX,
        }
    }

    fn write_property_entry(&mut self, entry: &EntryBuilder) -> u32 {
        let context_index = Self::index_of(self.contexts, &entry.context);
        let type_index = Self::index_of(self.types, &entry.type_);
        let offset = self.arena.allocate(PROPERTY_ENTRY_SIZE);
        let name_offset = self.arena.allocate_string(&entry.name);
        self.arena.write_u32(offset, name_offset);
        self.arena.write_u32(offset + 4, entry.name.len() as u32);
        self.arena.write_u32(offset + 8, context_index);
        self.arena.write_u32(offset + 12, type_index);
        offset
    }

    fn write_trie_node(&mut self, node: &BuilderNode) -> u32 {
        let trie = self.arena.allocate(TRIE_NODE_SIZE);
        let entry = self.write_property_entry(&node.entry);
        self.arena.write_u32(trie, entry);

        let mut prefixes = node.prefixes.clone();
        crate::cxx_sort::sort_by(&mut prefixes, |lhs, rhs| lhs.name.len() > rhs.name.len());
        self.arena.write_u32(trie + 12, prefixes.len() as u32);
        let prefix_array = self.arena.allocate(4 * prefixes.len());
        self.arena.write_u32(trie + 16, prefix_array);
        for (index, prefix) in prefixes.iter().enumerate() {
            let offset = self.write_property_entry(prefix);
            self.arena
                .write_u32(prefix_array + 4 * index as u32, offset);
        }

        let mut exact = node.exact_matches.clone();
        crate::cxx_sort::sort_by(&mut exact, |lhs, rhs| lhs.name < rhs.name);
        self.arena.write_u32(trie + 20, exact.len() as u32);
        let exact_array = self.arena.allocate(4 * exact.len());
        self.arena.write_u32(trie + 24, exact_array);
        for (index, entry) in exact.iter().enumerate() {
            let offset = self.write_property_entry(entry);
            self.arena.write_u32(exact_array + 4 * index as u32, offset);
        }

        let mut children = node.children.clone();
        crate::cxx_sort::sort_by(&mut children, |lhs, rhs| lhs.entry.name < rhs.entry.name);
        self.arena.write_u32(trie + 4, children.len() as u32);
        let children_array = self.arena.allocate(4 * children.len());
        self.arena.write_u32(trie + 8, children_array);
        for (index, child) in children.iter().enumerate() {
            let offset = self.write_trie_node(child);
            self.arena
                .write_u32(children_array + 4 * index as u32, offset);
        }
        trie
    }
}

/// `BuildTrie`: the exact bytes init writes to
/// `/dev/__properties__/property_info`.
pub fn build_trie(
    entries: &[PropertyInfoEntry],
    default_context: &str,
    default_type: &str,
) -> Result<Vec<u8>, String> {
    let mut builder = TrieBuilder::new(default_context, default_type);
    for entry in entries {
        builder.add(entry)?;
    }
    let mut serializer = Serializer {
        arena: Arena { data: Vec::new() },
        contexts: &builder.contexts,
        types: &builder.types,
    };
    let header = serializer.arena.allocate(HEADER_SIZE);
    serializer.arena.write_u32(header, 1); // current_version
    serializer.arena.write_u32(header + 4, 1); // minimum_supported_version
    let contexts_offset = serializer.arena.size();
    serializer.arena.write_u32(header + 12, contexts_offset);
    serializer.serialize_strings(&builder.contexts);
    let types_offset = serializer.arena.size();
    serializer.arena.write_u32(header + 16, types_offset);
    serializer.serialize_strings(&builder.types);
    let size = serializer.arena.size();
    serializer.arena.write_u32(header + 8, size);
    let root = serializer.write_trie_node(&builder.root);
    serializer.arena.write_u32(header + 20, root);
    let size = serializer.arena.size();
    serializer.arena.write_u32(header + 8, size);
    Ok(serializer.arena.data)
}

// ---------------------------------------------------------------------------
// PropertyInfoArea (libpropertyinfoparser)

/// A read-only view of serialized `property_info` bytes.
#[derive(Clone, Copy, Debug)]
pub struct PropertyInfoArea<'a> {
    data: &'a [u8],
}

#[derive(Clone, Copy, Debug)]
struct TrieNodeView {
    base: u32,
}

impl<'a> PropertyInfoArea<'a> {
    /// `PropertyInfoAreaFile::LoadPath` checks: minimum version 1 and
    /// `size` equal to the file size.
    pub fn new(data: &'a [u8]) -> Result<Self, String> {
        if data.len() < HEADER_SIZE {
            return Err("property_info is smaller than its header".to_string());
        }
        let area = Self { data };
        if area.u32_at(4) > 1 {
            return Err("property_info minimum_supported_version is newer than 1".to_string());
        }
        if area.u32_at(8) as usize != data.len() {
            return Err("property_info size does not match its length".to_string());
        }
        Ok(area)
    }

    fn u32_at(&self, offset: u32) -> u32 {
        let o = offset as usize;
        self.data
            .get(o..o + 4)
            .map_or(u32::MAX, |b| u32::from_ne_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn c_string(&self, offset: u32) -> &'a [u8] {
        let rest = &self.data[(offset as usize).min(self.data.len())..];
        let end = rest.iter().position(|b| *b == 0).unwrap_or(rest.len());
        &rest[..end]
    }

    pub fn current_version(&self) -> u32 {
        self.u32_at(0)
    }

    pub fn minimum_supported_version(&self) -> u32 {
        self.u32_at(4)
    }

    pub fn size(&self) -> u32 {
        self.u32_at(8)
    }

    pub fn num_contexts(&self) -> u32 {
        self.u32_at(self.u32_at(12))
    }

    pub fn num_types(&self) -> u32 {
        self.u32_at(self.u32_at(16))
    }

    pub fn context(&self, index: u32) -> &'a str {
        let array = self.u32_at(12) + 4;
        std::str::from_utf8(self.c_string(self.u32_at(array + 4 * index))).unwrap_or("")
    }

    pub fn type_name(&self, index: u32) -> &'a str {
        let array = self.u32_at(16) + 4;
        std::str::from_utf8(self.c_string(self.u32_at(array + 4 * index))).unwrap_or("")
    }

    /// All context strings, in index order (the prop_area file names).
    pub fn contexts(&self) -> Vec<&'a str> {
        (0..self.num_contexts()).map(|i| self.context(i)).collect()
    }

    fn root(&self) -> TrieNodeView {
        TrieNodeView {
            base: self.u32_at(20),
        }
    }

    fn node_entry(&self, node: TrieNodeView) -> u32 {
        self.u32_at(node.base)
    }

    fn entry_name(&self, entry: u32) -> &'a [u8] {
        self.c_string(self.u32_at(entry))
    }

    fn find_child(&self, node: TrieNodeView, name: &[u8]) -> Option<TrieNodeView> {
        let count = self.u32_at(node.base + 4) as i64;
        let array = self.u32_at(node.base + 8);
        let (mut bottom, mut top) = (0i64, count - 1);
        while top >= bottom {
            let search = (top + bottom) / 2;
            let child = TrieNodeView {
                base: self.u32_at(array + 4 * search as u32),
            };
            let child_name = self.entry_name(self.node_entry(child));
            // strncmp(child_name, name, namelen), then prefix-only match is
            // treated as "greater".
            let compare_len = name.len().min(child_name.len() + 1);
            let mut cmp = strncmp(child_name, name, compare_len);
            if cmp == 0 && child_name.len() > name.len() {
                cmp = 1;
            }
            match cmp {
                0 => return Some(child),
                c if c < 0 => bottom = search + 1,
                _ => top = search - 1,
            }
        }
        None
    }

    fn check_prefix_match(
        &self,
        remaining: &[u8],
        node: TrieNodeView,
        ctx: &mut u32,
        ty: &mut u32,
    ) {
        let count = self.u32_at(node.base + 12);
        let array = self.u32_at(node.base + 16);
        for i in 0..count {
            let entry = self.u32_at(array + 4 * i);
            let prefix_len = self.u32_at(entry + 4) as usize;
            if prefix_len > remaining.len() {
                continue;
            }
            let name = self.entry_name(entry);
            if strncmp(name, remaining, prefix_len) == 0 {
                if self.u32_at(entry + 8) != u32::MAX {
                    *ctx = self.u32_at(entry + 8);
                }
                if self.u32_at(entry + 12) != u32::MAX {
                    *ty = self.u32_at(entry + 12);
                }
                return;
            }
        }
    }

    /// `GetPropertyInfoIndexes`: `(context_index, type_index)`, each
    /// `u32::MAX` when nothing matched.
    pub fn property_info_indexes(&self, name: &str) -> (u32, u32) {
        let mut context_index = u32::MAX;
        let mut type_index = u32::MAX;
        let mut remaining = name.as_bytes();
        let mut node = self.root();
        loop {
            let entry = self.node_entry(node);
            if self.u32_at(entry + 8) != u32::MAX {
                context_index = self.u32_at(entry + 8);
            }
            if self.u32_at(entry + 12) != u32::MAX {
                type_index = self.u32_at(entry + 12);
            }
            self.check_prefix_match(remaining, node, &mut context_index, &mut type_index);
            let Some(sep) = remaining.iter().position(|b| *b == b'.') else {
                break;
            };
            let Some(child) = self.find_child(node, &remaining[..sep]) else {
                break;
            };
            node = child;
            remaining = &remaining[sep + 1..];
        }
        let count = self.u32_at(node.base + 20);
        let array = self.u32_at(node.base + 24);
        for i in 0..count {
            let entry = self.u32_at(array + 4 * i);
            if self.entry_name(entry) == remaining {
                let exact_context = self.u32_at(entry + 8);
                let exact_type = self.u32_at(entry + 12);
                return (
                    if exact_context != u32::MAX {
                        exact_context
                    } else {
                        context_index
                    },
                    if exact_type != u32::MAX {
                        exact_type
                    } else {
                        type_index
                    },
                );
            }
        }
        self.check_prefix_match(remaining, node, &mut context_index, &mut type_index);
        (context_index, type_index)
    }

    /// `GetPropertyInfo`: the context and type strings.
    pub fn property_info(&self, name: &str) -> (Option<&'a str>, Option<&'a str>) {
        let (context, type_) = self.property_info_indexes(name);
        (
            (context != u32::MAX).then(|| self.context(context)),
            (type_ != u32::MAX).then(|| self.type_name(type_)),
        )
    }
}

/// C `strncmp` over byte slices treated as NUL-terminated strings.
fn strncmp(a: &[u8], b: &[u8], n: usize) -> i32 {
    for i in 0..n {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        if x != y {
            return x as i32 - y as i32;
        }
        if x == 0 {
            return 0;
        }
    }
    0
}
