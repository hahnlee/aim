//! The resources a manifest's attributes resolve against while the
//! package is parsed: the APK's resource table and the framework's, each
//! value chosen for the parser's configuration as `AssetManager2` chooses
//! it (`FindEntry`, `ResTable_config::match` and `isBetterThan`), and
//! references followed as `ResolveReference` follows them.
//!
//! The parser's configuration is `ResourcesImpl`'s for PackageManager's
//! `Resources`: the default locale, the resources' SDK level and nothing
//! else (an undefined density, no screen size in dp). Locales are matched
//! by language, then preferring the requested region, then none, as the
//! original does for the locales a system image defaults to (it consults
//! CLDR's region parents, which this does not).
//!
//! Ported from the Android Open Source Project (`android-16.0.0_r1`,
//! `libs/androidfw`), Copyright (C) The Android Open Source Project,
//! Licensed under the Apache License, Version 2.0.

use std::collections::HashMap;

use aim_apps::res::{
    Result, STRING_POOL, Strings, TABLE, TABLE_PACKAGE, TABLE_TYPE, bad, chunks, u16_at, u32_at,
};

const TABLE_TYPE_SPEC: u16 = 0x0202;
const TABLE_OVERLAYABLE: u16 = 0x0204;

// `Res_value` types.
pub const TYPE_NULL: u8 = 0x00;
pub const TYPE_REFERENCE: u8 = 0x01;
pub const TYPE_ATTRIBUTE: u8 = 0x02;
pub const TYPE_STRING: u8 = 0x03;
pub const TYPE_FLOAT: u8 = 0x04;
pub const TYPE_DIMENSION: u8 = 0x05;
pub const TYPE_FRACTION: u8 = 0x06;
pub const TYPE_DYNAMIC_REFERENCE: u8 = 0x07;
pub const TYPE_FIRST_INT: u8 = 0x10;
pub const TYPE_INT_HEX: u8 = 0x11;
pub const TYPE_INT_BOOLEAN: u8 = 0x12;
pub const TYPE_FIRST_COLOR_INT: u8 = 0x1c;
pub const TYPE_LAST_COLOR_INT: u8 = 0x1f;
pub const TYPE_LAST_INT: u8 = 0x1f;
/// `Res_value::DATA_NULL_EMPTY`: `@empty`.
pub const DATA_NULL_EMPTY: u32 = 1;

/// A `ResTable_config`, every field (0 where a shorter one ends).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Config {
    pub mcc: u16,
    pub mnc: u16,
    pub language: [u8; 2],
    pub country: [u8; 2],
    pub orientation: u8,
    pub touchscreen: u8,
    pub density: u16,
    pub keyboard: u8,
    pub navigation: u8,
    pub input_flags: u8,
    pub grammatical_inflection: u8,
    pub screen_width: u16,
    pub screen_height: u16,
    pub sdk_version: u16,
    pub minor_version: u16,
    pub screen_layout: u8,
    pub ui_mode: u8,
    pub smallest_screen_width_dp: u16,
    pub screen_width_dp: u16,
    pub screen_height_dp: u16,
    pub screen_layout2: u8,
    pub color_mode: u8,
}

const MASK_LAYOUTDIR: u8 = 0xc0;
const MASK_SCREENSIZE: u8 = 0x0f;
const MASK_SCREENLONG: u8 = 0x30;
const SCREENSIZE_NORMAL: u8 = 0x02;
const MASK_SCREENROUND: u8 = 0x03;
const MASK_WIDE_COLOR_GAMUT: u8 = 0x03;
const MASK_HDR: u8 = 0x0c;
const MASK_UI_MODE_TYPE: u8 = 0x0f;
const MASK_UI_MODE_NIGHT: u8 = 0x30;
const MASK_KEYSHIDDEN: u8 = 0x03;
const MASK_NAVHIDDEN: u8 = 0x0c;
const KEYSHIDDEN_NO: u8 = 1;
const KEYSHIDDEN_SOFT: u8 = 3;
const DENSITY_MEDIUM: i32 = 160;
const DENSITY_ANY: i32 = 0xfffe;

impl Config {
    fn parse(c: &[u8]) -> Config {
        let size = (u32_at(c, 0).unwrap_or(0) as usize).min(c.len());
        let b = |at: usize| if at < size { c[at] } else { 0 };
        let s = |at: usize| u16::from_le_bytes([b(at), b(at + 1)]);
        Config {
            mcc: s(4),
            mnc: s(6),
            language: [b(8), b(9)],
            country: [b(10), b(11)],
            orientation: b(12),
            touchscreen: b(13),
            density: s(14),
            keyboard: b(16),
            navigation: b(17),
            input_flags: b(18),
            grammatical_inflection: b(19),
            screen_width: s(20),
            screen_height: s(22),
            sdk_version: s(24),
            minor_version: s(26),
            screen_layout: b(28),
            ui_mode: b(29),
            smallest_screen_width_dp: s(30),
            screen_width_dp: s(32),
            screen_height_dp: s(34),
            screen_layout2: b(48),
            color_mode: b(49),
        }
    }

    fn imsi(&self) -> bool {
        self.mcc != 0 || self.mnc != 0
    }

    fn locale(&self) -> bool {
        self.language != [0, 0] || self.country != [0, 0]
    }

    fn screen_type(&self) -> bool {
        self.orientation != 0 || self.touchscreen != 0 || self.density != 0
    }

    fn input(&self) -> bool {
        self.keyboard != 0 || self.navigation != 0 || self.input_flags != 0
    }

    fn screen_size(&self) -> bool {
        self.screen_width != 0 || self.screen_height != 0
    }

    fn version(&self) -> bool {
        self.sdk_version != 0 || self.minor_version != 0
    }

    fn screen_config(&self) -> bool {
        self.screen_layout != 0 || self.ui_mode != 0 || self.smallest_screen_width_dp != 0
    }

    fn screen_size_dp(&self) -> bool {
        self.screen_width_dp != 0 || self.screen_height_dp != 0
    }

    fn screen_config2(&self) -> bool {
        self.screen_layout2 != 0 || self.color_mode != 0
    }

    /// `ResTable_config::match`.
    pub fn matches(&self, settings: &Config) -> bool {
        if self.imsi()
            && ((self.mcc != 0 && self.mcc != settings.mcc)
                || (self.mnc != 0 && self.mnc != settings.mnc))
        {
            return false;
        }
        if self.locale() && self.language != settings.language {
            return false;
        }
        if self.grammatical_inflection != 0
            && self.grammatical_inflection != settings.grammatical_inflection
        {
            return false;
        }
        if self.screen_config() {
            let ne = |mask: u8, a: u8, b: u8| a & mask != 0 && a & mask != b & mask;
            let (sl, set) = (self.screen_layout, settings.screen_layout);
            if ne(MASK_LAYOUTDIR, sl, set)
                || sl & MASK_SCREENSIZE > set & MASK_SCREENSIZE
                || ne(MASK_SCREENLONG, sl, set)
                || ne(MASK_UI_MODE_TYPE, self.ui_mode, settings.ui_mode)
                || ne(MASK_UI_MODE_NIGHT, self.ui_mode, settings.ui_mode)
                || (self.smallest_screen_width_dp != 0
                    && self.smallest_screen_width_dp > settings.smallest_screen_width_dp)
            {
                return false;
            }
        }
        if self.screen_config2() {
            let ne = |mask: u8, a: u8, b: u8| a & mask != 0 && a & mask != b & mask;
            if ne(
                MASK_SCREENROUND,
                self.screen_layout2,
                settings.screen_layout2,
            ) || ne(MASK_HDR, self.color_mode, settings.color_mode)
                || ne(MASK_WIDE_COLOR_GAMUT, self.color_mode, settings.color_mode)
            {
                return false;
            }
        }
        if self.screen_size_dp()
            && ((self.screen_width_dp != 0 && self.screen_width_dp > settings.screen_width_dp)
                || (self.screen_height_dp != 0
                    && self.screen_height_dp > settings.screen_height_dp))
        {
            return false;
        }
        if self.screen_type()
            && ((self.orientation != 0 && self.orientation != settings.orientation)
                || (self.touchscreen != 0 && self.touchscreen != settings.touchscreen))
        {
            return false;
        }
        if self.input() {
            let keys = self.input_flags & MASK_KEYSHIDDEN;
            let set_keys = settings.input_flags & MASK_KEYSHIDDEN;
            if keys != 0
                && keys != set_keys
                && (keys != KEYSHIDDEN_NO || set_keys != KEYSHIDDEN_SOFT)
            {
                return false;
            }
            let nav = self.input_flags & MASK_NAVHIDDEN;
            if (nav != 0 && nav != settings.input_flags & MASK_NAVHIDDEN)
                || (self.keyboard != 0 && self.keyboard != settings.keyboard)
                || (self.navigation != 0 && self.navigation != settings.navigation)
            {
                return false;
            }
        }
        if self.screen_size()
            && ((self.screen_width != 0 && self.screen_width > settings.screen_width)
                || (self.screen_height != 0 && self.screen_height > settings.screen_height))
        {
            return false;
        }
        if self.version()
            && ((self.sdk_version != 0 && self.sdk_version > settings.sdk_version)
                || (self.minor_version != 0 && self.minor_version != settings.minor_version))
        {
            return false;
        }
        true
    }

    /// `isLocaleBetterThan`, for a request with a language and a region.
    fn locale_better(&self, o: &Config, r: &Config) -> bool {
        if !self.locale() && !o.locale() {
            return false;
        }
        if self.language != o.language {
            // One has the requested language, the other none.
            let us = |c: [u8; 2]| c == [0, 0] || c == r.country;
            if r.language == *b"en" && r.country == *b"US" {
                return if self.language != [0, 0] {
                    us(self.country)
                } else {
                    !us(o.country)
                };
            }
            return self.language != [0, 0];
        }
        // The same language: the requested region, then none.
        let rank = |c: [u8; 2]| match c {
            c if c == r.country => 2,
            [0, 0] => 1,
            _ => 0,
        };
        rank(self.country) > rank(o.country)
    }

    /// `ResTable_config::isBetterThan` with a requested configuration.
    pub fn better_than(&self, o: &Config, r: &Config) -> bool {
        if (self.imsi() || o.imsi()) && self.mcc != o.mcc && r.mcc != 0 {
            return self.mcc != 0;
        }
        if (self.imsi() || o.imsi()) && self.mnc != o.mnc && r.mnc != 0 {
            return self.mnc != 0;
        }
        if r.locale() && (self.locale() || o.locale()) && self.locale_better(o, r) {
            return true;
        }
        if (self.grammatical_inflection != 0 || o.grammatical_inflection != 0)
            && self.grammatical_inflection != o.grammatical_inflection
            && r.grammatical_inflection != 0
        {
            return self.grammatical_inflection != 0;
        }
        let (sl, osl, rsl) = (self.screen_layout, o.screen_layout, r.screen_layout);
        if (sl != 0 || osl != 0) && (sl ^ osl) & MASK_LAYOUTDIR != 0 && rsl & MASK_LAYOUTDIR != 0 {
            return sl & MASK_LAYOUTDIR > osl & MASK_LAYOUTDIR;
        }
        if (self.smallest_screen_width_dp != 0 || o.smallest_screen_width_dp != 0)
            && self.smallest_screen_width_dp != o.smallest_screen_width_dp
        {
            return self.smallest_screen_width_dp > o.smallest_screen_width_dp;
        }
        if self.screen_size_dp() || o.screen_size_dp() {
            let delta = |c: &Config| {
                let mut d = 0i32;
                if r.screen_width_dp != 0 {
                    d += r.screen_width_dp as i32 - c.screen_width_dp as i32;
                }
                if r.screen_height_dp != 0 {
                    d += r.screen_height_dp as i32 - c.screen_height_dp as i32;
                }
                d
            };
            if delta(self) != delta(o) {
                return delta(self) < delta(o);
            }
        }
        if sl != 0 || osl != 0 {
            if (sl ^ osl) & MASK_SCREENSIZE != 0 && rsl & MASK_SCREENSIZE != 0 {
                let (my, other) = (sl & MASK_SCREENSIZE, osl & MASK_SCREENSIZE);
                let (mut fmy, mut fother) = (my, other);
                if rsl & MASK_SCREENSIZE >= SCREENSIZE_NORMAL {
                    if fmy == 0 {
                        fmy = SCREENSIZE_NORMAL;
                    }
                    if fother == 0 {
                        fother = SCREENSIZE_NORMAL;
                    }
                }
                if fmy == fother {
                    return my != 0;
                }
                return fmy > fother;
            }
            if (sl ^ osl) & MASK_SCREENLONG != 0 && rsl & MASK_SCREENLONG != 0 {
                return sl & MASK_SCREENLONG != 0;
            }
        }
        if (self.screen_layout2 != 0 || o.screen_layout2 != 0)
            && (self.screen_layout2 ^ o.screen_layout2) & MASK_SCREENROUND != 0
            && r.screen_layout2 & MASK_SCREENROUND != 0
        {
            return self.screen_layout2 & MASK_SCREENROUND != 0;
        }
        if self.color_mode != 0 || o.color_mode != 0 {
            let x = self.color_mode ^ o.color_mode;
            if x & MASK_WIDE_COLOR_GAMUT != 0 && r.color_mode & MASK_WIDE_COLOR_GAMUT != 0 {
                return self.color_mode & MASK_WIDE_COLOR_GAMUT != 0;
            }
            if x & MASK_HDR != 0 && r.color_mode & MASK_HDR != 0 {
                return self.color_mode & MASK_HDR != 0;
            }
        }
        if self.orientation != o.orientation && r.orientation != 0 {
            return self.orientation != 0;
        }
        if self.ui_mode != 0 || o.ui_mode != 0 {
            let x = self.ui_mode ^ o.ui_mode;
            if x & MASK_UI_MODE_TYPE != 0 && r.ui_mode & MASK_UI_MODE_TYPE != 0 {
                return self.ui_mode & MASK_UI_MODE_TYPE != 0;
            }
            if x & MASK_UI_MODE_NIGHT != 0 && r.ui_mode & MASK_UI_MODE_NIGHT != 0 {
                return self.ui_mode & MASK_UI_MODE_NIGHT != 0;
            }
        }
        if self.screen_type() || o.screen_type() {
            if self.density != o.density {
                let or_medium = |d: u16| if d == 0 { DENSITY_MEDIUM } else { d as i32 };
                let (this, other) = (or_medium(self.density), or_medium(o.density));
                if this == DENSITY_ANY {
                    return true;
                } else if other == DENSITY_ANY {
                    return false;
                }
                let requested = match r.density as i32 {
                    0 | DENSITY_ANY => DENSITY_MEDIUM,
                    d => d,
                };
                let (mut h, mut l, mut bigger) = (this, other, true);
                if l > h {
                    std::mem::swap(&mut l, &mut h);
                    bigger = false;
                }
                return if h == requested {
                    bigger
                } else if l >= requested {
                    !bigger
                } else {
                    bigger
                };
            }
            if self.touchscreen != o.touchscreen && r.touchscreen != 0 {
                return self.touchscreen != 0;
            }
        }
        if self.input() || o.input() {
            let (keys, okeys) = (
                self.input_flags & MASK_KEYSHIDDEN,
                o.input_flags & MASK_KEYSHIDDEN,
            );
            let rkeys = r.input_flags & MASK_KEYSHIDDEN;
            if keys != okeys && rkeys != 0 {
                if keys == 0 {
                    return false;
                }
                if okeys == 0 || rkeys == keys {
                    return true;
                }
                if rkeys == okeys {
                    return false;
                }
            }
            let (nav, onav) = (
                self.input_flags & MASK_NAVHIDDEN,
                o.input_flags & MASK_NAVHIDDEN,
            );
            if nav != onav && r.input_flags & MASK_NAVHIDDEN != 0 {
                if nav == 0 {
                    return false;
                }
                if onav == 0 {
                    return true;
                }
            }
            if self.keyboard != o.keyboard && r.keyboard != 0 {
                return self.keyboard != 0;
            }
            if self.navigation != o.navigation && r.navigation != 0 {
                return self.navigation != 0;
            }
        }
        if self.screen_size() || o.screen_size() {
            let delta = |c: &Config| {
                let mut d = 0i32;
                if r.screen_width != 0 {
                    d += r.screen_width as i32 - c.screen_width as i32;
                }
                if r.screen_height != 0 {
                    d += r.screen_height as i32 - c.screen_height as i32;
                }
                d
            };
            if delta(self) != delta(o) {
                return delta(self) < delta(o);
            }
        }
        if self.version() || o.version() {
            if self.sdk_version != o.sdk_version && r.sdk_version != 0 {
                return self.sdk_version > o.sdk_version;
            }
            if self.minor_version != o.minor_version && r.minor_version != 0 {
                return self.minor_version != 0;
            }
        }
        false
    }
}

/// A resource's value in one configuration.
#[derive(Clone, Debug)]
pub enum Entry {
    Value(u8, u32),
    /// A bag: its parent and its (key, type, data)s.
    Bag(u32, Vec<(u32, u8, u32)>),
}

struct Type {
    config: Config,
    entries: HashMap<u16, Entry>,
}

#[derive(Default)]
struct TypeGroup {
    /// `ResTable_typeSpec`'s configuration-change flags per entry.
    spec_flags: Vec<u32>,
    types: Vec<Type>,
}

struct Package {
    id: u8,
    types: HashMap<u8, TypeGroup>,
    /// `<overlayable>`s: name and actor.
    overlayables: Vec<(String, String)>,
    /// Type names by id, and resource ids by type and entry name.
    type_names: HashMap<u8, String>,
    ids: HashMap<(String, String), u32>,
}

/// A resource table (`resources.arsc`).
pub struct Table {
    strings: Strings,
    packages: Vec<Package>,
}

const FLAG_SPARSE: u8 = 0x01;
const FLAG_OFFSET16: u8 = 0x02;
const ENTRY_COMPLEX: u16 = 0x0001;
const ENTRY_COMPACT: u16 = 0x0008;

impl Table {
    pub fn parse(data: &[u8]) -> Result<Table> {
        let top = chunks(data)
            .next()
            .ok_or_else(|| bad("empty resource table"))??;
        if top.kind != TABLE {
            return Err(bad("not a resource table"));
        }
        let mut strings = None;
        let mut packages = Vec::new();
        for c in chunks(&top.data[top.header..]) {
            let c = c?;
            match c.kind {
                STRING_POOL => strings = Some(Strings::parse(c.data)?),
                TABLE_PACKAGE => packages.push(package(c.data, c.header)?),
                _ => {}
            }
        }
        Ok(Table {
            strings: strings.ok_or_else(|| bad("table without strings"))?,
            packages,
        })
    }

    /// `ApkAssets.definesOverlayable`.
    pub fn defines_overlayable(&self) -> bool {
        self.packages.iter().any(|p| !p.overlayables.is_empty())
    }

    /// `AssetManager.getOverlayableMap` of each package, in package order.
    pub fn overlayables(&self) -> impl Iterator<Item = (&str, &str)> {
        let mut packages: Vec<&Package> = self.packages.iter().collect();
        packages.sort_by_key(|p| p.id);
        packages
            .into_iter()
            .flat_map(|p| p.overlayables.iter().map(|(n, a)| (n.as_str(), a.as_str())))
    }

    /// The attributes by name, public and private: what `android:<name>`
    /// stands for.
    pub fn attr_ids(&self) -> HashMap<String, u32> {
        self.packages
            .iter()
            .flat_map(|p| &p.ids)
            .filter(|((kind, _), _)| kind == "attr" || kind == "^attr-private")
            .fold(HashMap::new(), |mut m, ((_, name), &id)| {
                let e = m.entry(name.clone()).or_insert(id);
                *e = (*e).min(id);
                m
            })
    }

    /// Resource `type/name`'s id.
    pub fn id(&self, kind: &str, name: &str) -> Option<u32> {
        self.packages
            .iter()
            .find_map(|p| p.ids.get(&(kind.to_owned(), name.to_owned())).copied())
    }

    /// `getResourceTypeName`.
    pub fn type_name(&self, id: u32) -> Option<&str> {
        let pid = (id >> 24) as u8;
        let p = self
            .packages
            .iter()
            .find(|p| p.id == pid || (p.id == 0 && pid == SHARED_LIBRARY_ID))?;
        p.type_names.get(&((id >> 16) as u8)).map(String::as_str)
    }

    /// The global string pool's string `i`.
    pub fn string(&self, i: u32) -> Option<&str> {
        self.strings.get(i)
    }

    /// `FindEntry` within this table: resource `id`'s best entry for
    /// `config`, the type spec's flags for it, and the entry's
    /// configuration. A shared library's package (id 0) answers for the id
    /// the parser's `AssetManager` assigns it.
    fn find(&self, id: u32, config: &Config) -> Option<(&Entry, u32, Config)> {
        let pid = (id >> 24) as u8;
        let p = self
            .packages
            .iter()
            .find(|p| p.id == pid || (p.id == 0 && pid == SHARED_LIBRARY_ID))?;
        let group = p.types.get(&((id >> 16) as u8))?;
        let entry = (id & 0xffff) as u16;
        let flags = group.spec_flags.get(entry as usize).copied().unwrap_or(0);
        let mut best: Option<(&Entry, &Config)> = None;
        for t in &group.types {
            if !t.config.matches(config) {
                continue;
            }
            let Some(e) = t.entries.get(&entry) else {
                continue;
            };
            if best.is_none_or(|(_, b)| t.config.better_than(b, config)) {
                best = Some((e, &t.config));
            }
        }
        best.map(|(e, c)| (e, flags, *c))
    }

    /// Every resource by type and name.
    fn names(&self) -> impl Iterator<Item = (&(String, String), &u32)> {
        self.packages.iter().flat_map(|p| &p.ids)
    }
}

/// The package id `AssetManager2` assigns the first shared library it
/// loads after the framework.
const SHARED_LIBRARY_ID: u8 = 0x02;

/// A runtime resource id: a shared library's own references (package 0)
/// as `DynamicRefTable` rewrites them.
pub fn runtime_id(id: u32) -> u32 {
    if id != 0 && id >> 24 == 0 {
        (SHARED_LIBRARY_ID as u32) << 24 | id
    } else {
        id
    }
}

/// A static overlay of the framework (an immutable RRO, as
/// `OverlayConfig.createImmutableFrameworkIdmapsInZygote` has the zygote
/// load them): its table and its idmap, the framework's resources it
/// overlays by type and name.
pub struct Overlay {
    table: Table,
    map: HashMap<u32, u32>,
}

impl Overlay {
    pub fn new(target: &Table, table: Table) -> Overlay {
        let map = table
            .names()
            .filter_map(|((kind, name), &id)| Some((target.id(kind, name)?, id)))
            .collect();
        Overlay { table, map }
    }
}

fn package(c: &[u8], header: usize) -> Result<Package> {
    let id = u32_at(c, 8)? as u8;
    let pool_at = |at: usize| -> Result<Option<Strings>> {
        let off = u32_at(c, at)? as usize;
        if off == 0 || off >= c.len() {
            return Ok(None);
        }
        Strings::parse(&c[off..]).map(Some)
    };
    let type_strings = pool_at(268)?;
    let key_strings = pool_at(276)?;
    let mut types: HashMap<u8, TypeGroup> = HashMap::new();
    let mut type_names = HashMap::new();
    let mut ids = HashMap::new();
    let mut overlayables = Vec::new();
    for sub in chunks(&c[header..]) {
        let sub = sub?;
        let type_id = sub.data.get(8).copied().unwrap_or(0);
        let type_name = type_strings
            .as_ref()
            .and_then(|s| s.get((type_id as u32).wrapping_sub(1)))
            .unwrap_or_default()
            .to_owned();
        if sub.kind == TABLE_TYPE {
            for (entry, key) in keys(sub.data, sub.header)? {
                let name = key_strings.as_ref().and_then(|s| s.get(key));
                if let Some(name) = name {
                    let rid = (id as u32) << 24 | (type_id as u32) << 16 | entry as u32;
                    // A name a later type repeats (a staged API's) is the
                    // first one's.
                    ids.entry((type_name.clone(), name.to_owned()))
                        .or_insert(rid);
                }
            }
        }
        if sub.kind == TABLE_TYPE || sub.kind == TABLE_TYPE_SPEC {
            type_names.insert(type_id, type_name);
        }
        match sub.kind {
            TABLE_OVERLAYABLE => {
                let utf16 = |at: usize| -> String {
                    let units: Vec<u16> = (0..256)
                        .map_while(|i| u16_at(sub.data, at + 2 * i).ok().filter(|&u| u != 0))
                        .collect();
                    String::from_utf16_lossy(&units)
                };
                overlayables.push((utf16(8), utf16(8 + 512)));
            }
            TABLE_TYPE_SPEC => {
                let count = u32_at(sub.data, 12)? as usize;
                let flags = (0..count)
                    .map(|i| u32_at(sub.data, sub.header + 4 * i))
                    .collect::<Result<_>>()?;
                types.entry(sub.data[8]).or_default().spec_flags = flags;
            }
            TABLE_TYPE => {
                let t = type_chunk(sub.data, sub.header)?;
                types.entry(sub.data[8]).or_default().types.push(t);
            }
            _ => {}
        }
    }
    Ok(Package {
        id,
        types,
        overlayables,
        type_names,
        ids,
    })
}

/// The entries of a `ResTable_type` and their key names' indices.
fn keys(c: &[u8], header: usize) -> Result<Vec<(u16, u32)>> {
    let mut out = Vec::new();
    for (index, at) in offsets(c, header)? {
        let eflags = u16_at(c, at + 2)?;
        let key = if eflags & ENTRY_COMPACT != 0 {
            u16_at(c, at)? as u32
        } else {
            u32_at(c, at + 4)?
        };
        out.push((index, key));
    }
    Ok(out)
}

/// Where each present entry of a `ResTable_type` starts.
fn offsets(c: &[u8], header: usize) -> Result<Vec<(u16, usize)>> {
    let flags = *c.get(9).ok_or_else(|| bad("truncated type"))?;
    let count = u32_at(c, 12)? as usize;
    let start = u32_at(c, 16)? as usize;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let (index, offset) = if flags & FLAG_SPARSE != 0 {
            (
                u16_at(c, header + 4 * i)?,
                u16_at(c, header + 4 * i + 2)? as usize * 4,
            )
        } else if flags & FLAG_OFFSET16 != 0 {
            match u16_at(c, header + 2 * i)? {
                0xffff => continue,
                o => (i as u16, o as usize * 4),
            }
        } else {
            match u32_at(c, header + 4 * i)? {
                u32::MAX => continue,
                o => (i as u16, o as usize),
            }
        };
        out.push((index, start + offset));
    }
    Ok(out)
}

/// A `ResTable_type`: its configuration and entries.
fn type_chunk(c: &[u8], header: usize) -> Result<Type> {
    let config = Config::parse(&c[20..header.max(20)]);
    let mut entries = HashMap::new();
    for (index, at) in offsets(c, header)? {
        let size = u16_at(c, at)? as usize;
        let eflags = u16_at(c, at + 2)?;
        let entry = if eflags & ENTRY_COMPACT != 0 {
            Entry::Value((eflags >> 8) as u8, u32_at(c, at + 4)?)
        } else if eflags & ENTRY_COMPLEX != 0 {
            let parent = u32_at(c, at + 8)?;
            let n = u32_at(c, at + 12)? as usize;
            let mut map = Vec::with_capacity(n);
            for k in 0..n {
                let m = at + size + 12 * k;
                let kind = *c.get(m + 7).ok_or_else(|| bad("truncated map"))?;
                map.push((u32_at(c, m)?, kind, u32_at(c, m + 8)?));
            }
            Entry::Bag(parent, map)
        } else {
            let v = at + size;
            let kind = *c.get(v + 3).ok_or_else(|| bad("truncated value"))?;
            Entry::Value(kind, u32_at(c, v + 4)?)
        };
        entries.insert(index, entry);
    }
    Ok(Type { config, entries })
}

/// A value an attribute resolved to (`AssetManager2::SelectedValue`).
#[derive(Clone, Debug, PartialEq)]
pub struct Selected {
    pub kind: u8,
    pub data: u32,
    /// The table whose string pool a string is in; `None` for the XML's.
    pub table: Option<usize>,
    /// The last resource followed, or 0.
    pub resid: u32,
    /// Native configuration-change flags of the resources followed.
    pub flags: u32,
}

/// The tables the parser's `AssetManager` holds, framework first, and the
/// framework's static overlays.
pub struct Resources<'a> {
    pub tables: Vec<&'a Table>,
    pub overlays: &'a [Overlay],
    pub config: Config,
}

impl Resources<'_> {
    /// The table a `Selected` names: one of the tables, then the
    /// overlays'.
    fn table(&self, i: usize) -> Option<&Table> {
        self.tables
            .get(i)
            .copied()
            .or_else(|| Some(&self.overlays.get(i - self.tables.len())?.table))
    }

    /// `FindEntry`: resource `id`'s value, the table it is in, and its
    /// flags; a framework resource as its overlays leave it.
    fn find(&self, id: u32) -> Option<(usize, &Entry, u32)> {
        let (mut t, (mut entry, flags, mut config)) = self
            .tables
            .iter()
            .enumerate()
            .find_map(|(i, table)| Some((i, table.find(id, &self.config)?)))?;
        if t == 0 {
            for (i, o) in self.overlays.iter().enumerate() {
                let Some(&oid) = o.map.get(&id) else { continue };
                let Some((e, _, c)) = o.table.find(oid, &self.config) else {
                    continue;
                };
                if c.better_than(&config, &self.config) || c == config {
                    (t, entry, config) = (self.tables.len() + i, e, c);
                }
            }
        }
        Some((t, entry, flags))
    }

    /// `ResolveReference`: a reference followed to its value.
    pub fn resolve(&self, v: &mut Selected) {
        if v.kind != TYPE_REFERENCE || v.data == 0 {
            return;
        }
        let mut combined = 0;
        let mut resid = runtime_id(v.data);
        for i in 0.. {
            let Some((table, entry, flags)) = self.find(resid) else {
                v.resid = resid;
                return;
            };
            let (kind, data) = match *entry {
                // A bag stands for itself.
                Entry::Bag(..) => (TYPE_REFERENCE, resid),
                Entry::Value(TYPE_DYNAMIC_REFERENCE, d) => (TYPE_REFERENCE, runtime_id(d)),
                Entry::Value(TYPE_REFERENCE, d) => (TYPE_REFERENCE, runtime_id(d)),
                Entry::Value(k, d) => (k, d),
            };
            *v = Selected {
                kind,
                data,
                table: Some(table),
                resid,
                flags: flags | combined,
            };
            if kind != TYPE_REFERENCE || data == 0 || data == resid || i == 20 {
                return;
            }
            combined = flags;
            resid = data;
        }
    }

    /// `getResourceTypeName`.
    pub fn type_name(&self, id: u32) -> Option<&str> {
        self.tables.iter().find_map(|t| t.type_name(runtime_id(id)))
    }

    /// `getStringArray`: a string array's strings (an item that is not a
    /// string is null).
    pub fn string_array(&self, id: u32) -> Option<Vec<Option<String>>> {
        let (t, entry, _) = self.find(runtime_id(id))?;
        let Entry::Bag(_, items) = entry else {
            return None;
        };
        Some(
            items
                .iter()
                .map(|&(_, kind, data)| {
                    let mut v = Selected {
                        kind,
                        data,
                        table: Some(t),
                        resid: 0,
                        flags: 0,
                    };
                    self.resolve(&mut v);
                    match (v.kind, v.table) {
                        (TYPE_STRING, Some(t)) => self.string(t, v.data).map(str::to_owned),
                        _ => None,
                    }
                })
                .collect(),
        )
    }

    /// `getString`: a string resource's string.
    pub fn resource_string(&self, id: u32) -> Option<String> {
        let mut v = Selected {
            kind: TYPE_REFERENCE,
            data: id,
            table: None,
            resid: 0,
            flags: 0,
        };
        self.resolve(&mut v);
        match (v.kind, v.table) {
            (TYPE_STRING, Some(t)) => self.string(t, v.data).map(str::to_owned),
            _ => None,
        }
    }

    /// The string of a resolved `TYPE_STRING` value from a table.
    pub fn string(&self, table: usize, i: u32) -> Option<&str> {
        self.table(table)?.string(i)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chooses_configurations_as_the_original() {
        let r = Config {
            language: *b"en",
            country: *b"US",
            sdk_version: 36,
            ..Config::default()
        };
        let default = Config::default();
        let v31 = Config {
            sdk_version: 31,
            ..Config::default()
        };
        let v37 = Config {
            sdk_version: 37,
            ..Config::default()
        };
        let en_gb = Config {
            language: *b"en",
            country: *b"GB",
            ..Config::default()
        };
        let fr = Config {
            language: *b"fr",
            ..Config::default()
        };
        let sw600 = Config {
            smallest_screen_width_dp: 600,
            ..Config::default()
        };
        assert!(v31.matches(&r) && !v37.matches(&r) && !fr.matches(&r) && !sw600.matches(&r));
        assert!(v31.better_than(&default, &r) && !default.better_than(&v31, &r));
        // For US English, the default beats another region's English.
        assert!(en_gb.matches(&r) && default.better_than(&en_gb, &r));
    }
}
