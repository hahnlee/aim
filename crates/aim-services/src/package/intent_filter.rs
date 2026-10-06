//! `IntentFilter` and how it matches an intent, as `android.content.IntentFilter`
//! (with `android.os.PatternMatcher`, `UriRelativeFilter` and
//! `UriRelativeFilterGroup`) matches at `android-16.0.0_r1`, where the
//! `relative_reference_intent_filters` flag is on. Strings are compared as
//! Java compares them, by UTF-16 unit.

use aim_android_xml::Element;
use aim_binder_host::parcel::{BAD_VALUE, Parcel, Reader, Result};

use super::uri::{Uri, parse_int};

/// How a parcel's strings are read: plainly, or through the
/// `Parcel.ReadWriteHelper` of the parcel's writer (the parser cache's
/// `PackageParserCacheHelper` string pool).
pub trait Strings {
    /// `readString`, `readString16`.
    fn string16(&mut self, r: &mut Reader<'_>) -> Result<Option<String>>;
    /// `readString8`.
    fn string8(&mut self, r: &mut Reader<'_>) -> Result<Option<String>>;
}

/// The strings of a plain parcel.
pub struct Plain;

impl Strings for Plain {
    fn string16(&mut self, r: &mut Reader<'_>) -> Result<Option<String>> {
        r.read_string16()
    }

    fn string8(&mut self, r: &mut Reader<'_>) -> Result<Option<String>> {
        r.read_string8()
    }
}

pub const MATCH_CATEGORY_MASK: i32 = 0xfff0000;
pub const MATCH_ADJUSTMENT_MASK: i32 = 0x000ffff;
pub const MATCH_ADJUSTMENT_NORMAL: i32 = 0x8000;
pub const MATCH_CATEGORY_EMPTY: i32 = 0x0100000;
pub const MATCH_CATEGORY_SCHEME: i32 = 0x0200000;
pub const MATCH_CATEGORY_HOST: i32 = 0x0300000;
pub const MATCH_CATEGORY_PORT: i32 = 0x0400000;
pub const MATCH_CATEGORY_PATH: i32 = 0x0500000;
pub const MATCH_CATEGORY_SCHEME_SPECIFIC_PART: i32 = 0x0580000;
pub const MATCH_CATEGORY_TYPE: i32 = 0x0600000;
pub const NO_MATCH_TYPE: i32 = -1;
pub const NO_MATCH_DATA: i32 = -2;
pub const NO_MATCH_ACTION: i32 = -3;
pub const NO_MATCH_CATEGORY: i32 = -4;
pub const NO_MATCH_EXTRAS: i32 = -5;

/// `IntentFilter.VISIBILITY_*`.
pub const VISIBILITY_NONE: i32 = 0;
pub const VISIBILITY_EXPLICIT: i32 = 1;

const WILDCARD: &str = "*";
const WILDCARD_PATH: &str = "/*";

pub const ACTION_VIEW: &str = "android.intent.action.VIEW";
pub const CATEGORY_BROWSABLE: &str = "android.intent.category.BROWSABLE";
pub const CATEGORY_APP_BROWSER: &str = "android.intent.category.APP_BROWSER";

/// `PatternMatcher.PATTERN_*`.
pub const PATTERN_LITERAL: i32 = 0;
pub const PATTERN_PREFIX: i32 = 1;
pub const PATTERN_SIMPLE_GLOB: i32 = 2;
pub const PATTERN_ADVANCED_GLOB: i32 = 3;
pub const PATTERN_SUFFIX: i32 = 4;

/// `PatternMatcher`'s parsed tokens of an advanced glob.
const PARSED_TOKEN_CHAR_SET_START: i32 = -1;
const PARSED_TOKEN_CHAR_SET_INVERSE_START: i32 = -2;
const PARSED_TOKEN_CHAR_SET_STOP: i32 = -3;
const PARSED_TOKEN_CHAR_ANY: i32 = -4;
const PARSED_MODIFIER_RANGE_START: i32 = -5;
const PARSED_MODIFIER_RANGE_STOP: i32 = -6;
const PARSED_MODIFIER_ZERO_OR_MORE: i32 = -7;
const PARSED_MODIFIER_ONE_OR_MORE: i32 = -8;
const MAX_PATTERN_STORAGE: usize = 2048;

/// `android.os.PatternMatcher`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PatternMatcher {
    pub pattern: String,
    pub kind: i32,
    /// An advanced glob's parsed form, which a parcel carries.
    parsed: Option<Vec<i32>>,
}

impl PatternMatcher {
    /// `new PatternMatcher(pattern, type)`; an error for an advanced glob
    /// the original refuses.
    pub fn new(pattern: &str, kind: i32) -> std::result::Result<PatternMatcher, String> {
        let parsed = if kind == PATTERN_ADVANCED_GLOB {
            Some(parse_advanced(&utf16(pattern))?)
        } else {
            None
        };
        Ok(PatternMatcher {
            pattern: pattern.to_owned(),
            kind,
            parsed,
        })
    }

    /// `PatternMatcher(Parcel)`.
    pub fn read(r: &mut Reader<'_>, strings: &mut dyn Strings) -> Result<PatternMatcher> {
        let pattern = strings.string16(r)?.ok_or(BAD_VALUE)?;
        let kind = r.read_i32()?;
        let parsed = read_int_array(r)?;
        Ok(PatternMatcher {
            pattern,
            kind,
            parsed,
        })
    }

    pub(crate) fn write_cache(&self, writer: &mut super::parse::parcel::Writer) {
        writer.string(Some(&self.pattern));
        writer.int(self.kind);
        writer.ints(self.parsed.as_deref());
    }

    /// `writeToParcel`.
    pub fn write(&self, p: &mut Parcel) {
        p.write_string16(Some(&self.pattern));
        p.write_i32(self.kind);
        match &self.parsed {
            Some(parsed) => {
                p.write_i32(parsed.len() as i32);
                parsed.iter().for_each(|&v| p.write_i32(v));
            }
            None => p.write_i32(-1),
        }
    }

    /// `match`.
    pub fn matches(&self, s: Option<&str>) -> bool {
        let Some(s) = s else { return false };
        match self.kind {
            PATTERN_LITERAL => self.pattern == s,
            PATTERN_PREFIX => s.starts_with(self.pattern.as_str()),
            PATTERN_SIMPLE_GLOB => match_glob(&utf16(&self.pattern), &utf16(s)),
            PATTERN_ADVANCED_GLOB => {
                match_advanced(self.parsed.as_deref().unwrap_or_default(), &utf16(s))
            }
            PATTERN_SUFFIX => s.ends_with(self.pattern.as_str()),
            _ => false,
        }
    }
}

fn utf16(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

/// `Parcel.createIntArray`.
fn read_int_array(r: &mut Reader<'_>) -> Result<Option<Vec<i32>>> {
    let n = r.read_i32()?;
    if n < 0 {
        return Ok(None);
    }
    if n as usize > r.remaining() / 4 {
        return Err(BAD_VALUE);
    }
    (0..n)
        .map(|_| r.read_i32())
        .collect::<Result<_>>()
        .map(Some)
}

/// `Parcel.readStringList` into a new list.
fn read_string_list(r: &mut Reader<'_>, strings: &mut dyn Strings) -> Result<Vec<String>> {
    let n = r.read_i32()?;
    let mut list = Vec::new();
    for _ in 0..n.max(0) {
        // A null element is kept by the original; no filter has one.
        list.push(strings.string16(r)?.unwrap_or_default());
    }
    Ok(list)
}

/// `PatternMatcher.matchGlobPattern`: `.` is any character, `*` repeats
/// the one before it, `\` escapes.
fn match_glob(pattern: &[u16], m: &[u16]) -> bool {
    let np = pattern.len();
    if np == 0 {
        return m.is_empty();
    }
    let nm = m.len();
    let at = |i: usize| if i < np { pattern[i] } else { 0 };
    let (dot, star, backslash) = (b'.' as u16, b'*' as u16, b'\\' as u16);
    let (mut ip, mut im) = (0, 0);
    let mut next = pattern[0];
    while ip < np && im < nm {
        let mut c = next;
        ip += 1;
        next = at(ip);
        let escaped = c == backslash;
        if escaped {
            c = next;
            ip += 1;
            next = at(ip);
        }
        if next == star {
            if !escaped && c == dot {
                if ip >= np - 1 {
                    // A trailing `.*` matches the rest.
                    return true;
                }
                ip += 1;
                next = pattern[ip];
                if next == backslash {
                    ip += 1;
                    next = at(ip);
                }
                // Consume up to the pattern's next character.
                while m[im] != next {
                    im += 1;
                    if im == nm {
                        return false;
                    }
                }
                ip += 1;
                next = at(ip);
                im += 1;
            } else {
                // Consume only characters equal to the one before `*`.
                while m[im] == c {
                    im += 1;
                    if im == nm {
                        break;
                    }
                }
                ip += 1;
                next = at(ip);
            }
        } else {
            if c != dot && m[im] != c {
                return false;
            }
            im += 1;
        }
    }
    if ip >= np && im >= nm {
        return true;
    }
    // The match string ended with a `.*` left in the pattern.
    ip + 2 == np && pattern[ip] == dot && pattern[ip + 1] == star
}

/// `PatternMatcher.parseAndVerifyAdvancedPattern`.
fn parse_advanced(pattern: &[u16]) -> std::result::Result<Vec<i32>, String> {
    let lp = pattern.len();
    let mut out: Vec<i32> = Vec::new();
    let (mut ip, mut in_set, mut in_range, mut in_char_class) = (0, false, false, false);
    let is_modifier = |t: i32| {
        matches!(
            t,
            PARSED_MODIFIER_ONE_OR_MORE
                | PARSED_MODIFIER_ZERO_OR_MORE
                | PARSED_MODIFIER_RANGE_STOP
                | PARSED_MODIFIER_RANGE_START
        )
    };
    let after_token = |out: &[i32]| match out.last() {
        Some(&t) if !is_modifier(t) => Ok(()),
        _ => Err("Modifier must follow a token.".to_owned()),
    };
    while ip < lp {
        if out.len() > MAX_PATTERN_STORAGE - 3 {
            return Err("Pattern is too large!".into());
        }
        let mut c = pattern[ip];
        let mut add = false;
        match c {
            0x5b /* [ */ => {
                if in_set {
                    add = true;
                } else {
                    if pattern.get(ip + 1) == Some(&0x5e) {
                        out.push(PARSED_TOKEN_CHAR_SET_INVERSE_START);
                        ip += 1;
                    } else {
                        out.push(PARSED_TOKEN_CHAR_SET_START);
                    }
                    ip += 1;
                    in_set = true;
                    continue;
                }
            }
            0x5d /* ] */ => {
                if !in_set {
                    add = true;
                } else {
                    if matches!(
                        out.last(),
                        Some(&(PARSED_TOKEN_CHAR_SET_START | PARSED_TOKEN_CHAR_SET_INVERSE_START))
                    ) {
                        return Err("You must define characters in a set.".into());
                    }
                    out.push(PARSED_TOKEN_CHAR_SET_STOP);
                    in_set = false;
                    in_char_class = false;
                }
            }
            0x7b /* { */ => {
                if !in_set {
                    after_token(&out)?;
                    out.push(PARSED_MODIFIER_RANGE_START);
                    ip += 1;
                    in_range = true;
                }
            }
            0x7d /* } */ => {
                if in_range {
                    out.push(PARSED_MODIFIER_RANGE_STOP);
                    in_range = false;
                }
            }
            0x2a /* * */ => {
                if !in_set {
                    after_token(&out)?;
                    out.push(PARSED_MODIFIER_ZERO_OR_MORE);
                }
            }
            0x2b /* + */ => {
                if !in_set {
                    after_token(&out)?;
                    out.push(PARSED_MODIFIER_ONE_OR_MORE);
                }
            }
            0x2e /* . */ => {
                if !in_set {
                    out.push(PARSED_TOKEN_CHAR_ANY);
                }
            }
            0x5c /* \ */ => {
                if ip + 1 >= lp {
                    return Err("Escape found at end of pattern!".into());
                }
                ip += 1;
                c = pattern[ip];
                add = true;
            }
            _ => add = true,
        }
        if in_set {
            if in_char_class {
                out.push(c as i32);
                in_char_class = false;
            } else if ip + 2 < lp && pattern[ip + 1] == 0x2d && pattern[ip + 2] != 0x5d {
                // The lower end of a range.
                in_char_class = true;
                out.push(c as i32);
                ip += 1;
            } else {
                out.push(c as i32);
                out.push(c as i32);
            }
        } else if in_range {
            let end = pattern[ip..]
                .iter()
                .position(|&c| c == 0x7d)
                .map(|i| ip + i)
                .ok_or("Range not ended with '}'")?;
            let range = String::from_utf16_lossy(&pattern[ip..end]);
            let bad = || "Range number format incorrect".to_owned();
            let (min, max) = match range.find(',') {
                None => {
                    let n = parse_int(&range).ok_or_else(bad)?;
                    (n, n)
                }
                Some(comma) => {
                    let min = parse_int(&range[..comma]).ok_or_else(bad)?;
                    let max = if comma == range.len() - 1 {
                        i32::MAX
                    } else {
                        parse_int(&range[comma + 1..]).ok_or_else(bad)?
                    };
                    (min, max)
                }
            };
            if min > max {
                return Err("Range quantifier minimum is greater than maximum".into());
            }
            out.push(min);
            out.push(max);
            ip = end;
            continue;
        } else if add {
            out.push(c as i32);
        }
        ip += 1;
    }
    if in_set {
        return Err("Set was not terminated!".into());
    }
    Ok(out)
}

/// `PatternMatcher.matchAdvancedPattern`.
fn match_advanced(parsed: &[i32], m: &[u16]) -> bool {
    #[derive(Clone, Copy)]
    enum Token {
        Any,
        Set,
        InverseSet,
        Literal,
    }
    let (lp, lm) = (parsed.len(), m.len());
    let (mut ip, mut im) = (0, 0);
    let (mut set_start, mut set_end) = (0, 0);
    while ip < lp {
        let token = match parsed[ip] {
            PARSED_TOKEN_CHAR_ANY => {
                ip += 1;
                Token::Any
            }
            t @ (PARSED_TOKEN_CHAR_SET_START | PARSED_TOKEN_CHAR_SET_INVERSE_START) => {
                set_start = ip + 1;
                ip += 1;
                while ip < lp && parsed[ip] != PARSED_TOKEN_CHAR_SET_STOP {
                    ip += 1;
                }
                set_end = ip - 1;
                ip += 1;
                if t == PARSED_TOKEN_CHAR_SET_START {
                    Token::Set
                } else {
                    Token::InverseSet
                }
            }
            _ => {
                set_start = ip;
                ip += 1;
                Token::Literal
            }
        };
        let (min, max) = match parsed.get(ip) {
            Some(&PARSED_MODIFIER_ZERO_OR_MORE) => {
                ip += 1;
                (0, i32::MAX)
            }
            Some(&PARSED_MODIFIER_ONE_OR_MORE) => {
                ip += 1;
                (1, i32::MAX)
            }
            Some(&PARSED_MODIFIER_RANGE_START) => {
                let range = (parsed[ip + 1], parsed[ip + 2]);
                ip += 4;
                range
            }
            _ => (1, 1),
        };
        if min > max {
            return false;
        }
        let in_set = |c: u16| {
            (set_start..set_end)
                .step_by(2)
                .any(|i| c as i32 >= parsed[i] && c as i32 <= parsed[i + 1])
        };
        let matches = |at: usize| {
            at < lm
                && match token {
                    Token::Any => true,
                    Token::Set => in_set(m[at]),
                    Token::InverseSet => !in_set(m[at]),
                    Token::Literal => m[at] as i32 == parsed[set_start],
                }
        };
        let mut matched = 0;
        while matched < max && matches(im + matched as usize) {
            matched += 1;
        }
        if matched < min {
            return false;
        }
        im += matched as usize;
    }
    ip >= lp && im >= lm
}

/// `String.compareToIgnoreCase(...) == 0`, by UTF-16 unit.
fn equals_ignore_case(a: &[u16], b: &[u16]) -> bool {
    let fold = |c: u16| -> u32 {
        char::from_u32(c as u32).map_or(c as u32, |c| {
            let upper = simple(c.to_uppercase()).unwrap_or(c);
            simple(upper.to_lowercase()).unwrap_or(upper) as u32
        })
    };
    a.len() == b.len() && a.iter().zip(b).all(|(&x, &y)| x == y || fold(x) == fold(y))
}

/// A case mapping's single character (`Character.toUpperCase(char)`
/// maps only to one).
fn simple(mut it: impl Iterator<Item = char>) -> Option<char> {
    let c = it.next()?;
    it.next().is_none().then_some(c)
}

/// `IntentFilter.AuthorityEntry`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthorityEntry {
    pub orig_host: String,
    /// Without a leading `*` of a wildcard host.
    pub host: String,
    pub wild: bool,
    pub port: i32,
}

impl AuthorityEntry {
    pub fn new(host: &str, port: Option<&str>) -> AuthorityEntry {
        let wild = host.starts_with('*');
        AuthorityEntry {
            orig_host: host.to_owned(),
            host: if wild { &host[1..] } else { host }.to_owned(),
            wild,
            port: port.and_then(parse_int).unwrap_or(-1),
        }
    }

    fn read(r: &mut Reader<'_>, strings: &mut dyn Strings) -> Result<AuthorityEntry> {
        Ok(AuthorityEntry {
            orig_host: strings.string16(r)?.unwrap_or_default(),
            host: strings.string16(r)?.unwrap_or_default(),
            wild: r.read_i32()? != 0,
            port: r.read_i32()?,
        })
    }

    fn write(&self, p: &mut Parcel) {
        p.write_string16(Some(&self.orig_host));
        p.write_string16(Some(&self.host));
        p.write_i32(self.wild as i32);
        p.write_i32(self.port);
    }

    /// `match(Uri, boolean)`.
    pub fn matches(&self, data: &Uri, wildcards: bool) -> i32 {
        let Some(host) = data.host() else {
            return if wildcards && self.wild && self.host.is_empty() {
                MATCH_CATEGORY_HOST
            } else {
                NO_MATCH_DATA
            };
        };
        if !wildcards || host != WILDCARD {
            let host = utf16(&host);
            let mine = utf16(&self.host);
            let host = if self.wild {
                if host.len() < mine.len() {
                    return NO_MATCH_DATA;
                }
                &host[host.len() - mine.len()..]
            } else {
                &host[..]
            };
            if !equals_ignore_case(host, &mine) {
                return NO_MATCH_DATA;
            }
        }
        // With wildcards, ports are ignored.
        if !wildcards && self.port >= 0 {
            if self.port != data.port() {
                return NO_MATCH_DATA;
            }
            return MATCH_CATEGORY_PORT;
        }
        MATCH_CATEGORY_HOST
    }
}

/// `UriRelativeFilter.PATH`, `QUERY`, `FRAGMENT`.
pub const URI_PART_PATH: i32 = 0;
pub const URI_PART_QUERY: i32 = 1;
pub const URI_PART_FRAGMENT: i32 = 2;
/// `UriRelativeFilterGroup.ACTION_ALLOW`.
pub const ACTION_ALLOW: i32 = 0;

/// `UriRelativeFilter`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UriRelativeFilter {
    pub uri_part: i32,
    pub pattern_type: i32,
    pub filter: String,
}

impl UriRelativeFilter {
    fn matches(&self, data: &Uri) -> bool {
        // The original builds its matcher here, and throws for a bad
        // advanced glob.
        let Ok(pe) = PatternMatcher::new(&self.filter, self.pattern_type) else {
            return false;
        };
        match self.uri_part {
            URI_PART_PATH => pe.matches(data.path().as_deref()),
            URI_PART_QUERY => data.query().is_some_and(|query| {
                let mut params = java_split(&query, '&');
                if params.len() == 1 {
                    params = java_split(&query, ';');
                }
                params.iter().any(|p| pe.matches(Some(p)))
            }),
            URI_PART_FRAGMENT => pe.matches(data.fragment().as_deref()),
            _ => false,
        }
    }
}

/// `String.split` by one character: trailing empty strings dropped, one
/// empty string for an empty input.
fn java_split(s: &str, by: char) -> Vec<&str> {
    let mut parts: Vec<&str> = s.split(by).collect();
    while parts.len() > 1 && parts.last() == Some(&"") {
        parts.pop();
    }
    if parts.len() == 1 && parts[0].is_empty() && !s.is_empty() {
        parts.clear();
    }
    parts
}

/// `UriRelativeFilterGroup`: matches when all its filters do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UriRelativeFilterGroup {
    pub action: i32,
    pub filters: Vec<UriRelativeFilter>,
}

impl UriRelativeFilterGroup {
    pub fn new(action: i32) -> UriRelativeFilterGroup {
        UriRelativeFilterGroup {
            action,
            filters: Vec::new(),
        }
    }

    /// `addUriRelativeFilter`: a set.
    pub fn add(&mut self, uri_part: i32, pattern_type: i32, filter: &str) {
        let f = UriRelativeFilter {
            uri_part,
            pattern_type,
            filter: filter.to_owned(),
        };
        if !self.filters.contains(&f) {
            self.filters.push(f);
            self.filters.sort_by_key(|f| {
                31i32.wrapping_add(f.uri_part).wrapping_mul(31)
                    .wrapping_add(f.pattern_type).wrapping_mul(31)
                    .wrapping_add(crate::package::info::java_hash(&f.filter))
            });
        }
    }

    fn read(r: &mut Reader<'_>, strings: &mut dyn Strings) -> Result<UriRelativeFilterGroup> {
        let mut group = UriRelativeFilterGroup::new(r.read_i32()?);
        for _ in 0..r.read_i32()?.max(0) {
            let uri_part = r.read_i32()?;
            let pattern_type = r.read_i32()?;
            let filter = strings.string16(r)?.unwrap_or_default();
            group.add(uri_part, pattern_type, &filter);
        }
        Ok(group)
    }

    fn write(&self, p: &mut Parcel) {
        p.write_i32(self.action);
        p.write_i32(self.filters.len() as i32);
        for f in &self.filters {
            p.write_i32(f.uri_part);
            p.write_i32(f.pattern_type);
            p.write_string16(Some(&f.filter));
        }
    }

    fn parse(e: &Element) -> UriRelativeFilterGroup {
        let int = |e: &Element, name| e.string(name).and_then(|v| parse_int(&v)).unwrap_or(0);
        let mut group = UriRelativeFilterGroup::new(int(e, "allow"));
        for f in e.children().filter(|f| f.name == "uriRelativeFilter") {
            let filter = f.string("filter").unwrap_or_default();
            group.add(int(f, "part"), int(f, "pattern"), &filter);
        }
        group
    }

    /// `matchGroupsToUri`: the first group that matches decides.
    pub fn match_groups(groups: &[UriRelativeFilterGroup], data: &Uri) -> bool {
        groups
            .iter()
            .find(|g| g.matches(data))
            .is_some_and(|g| g.action == ACTION_ALLOW)
    }

    fn matches(&self, data: &Uri) -> bool {
        !self.filters.is_empty() && self.filters.iter().all(|f| f.matches(data))
    }
}

/// `android.content.IntentFilter`. A list the original keeps null is
/// `None` here, since matching tells null from empty.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct IntentFilter {
    /// An `ArraySet`: no duplicates.
    pub actions: Vec<String>,
    pub categories: Option<Vec<String>>,
    pub schemes: Option<Vec<String>>,
    pub ssps: Option<Vec<PatternMatcher>>,
    pub authorities: Option<Vec<AuthorityEntry>>,
    pub paths: Option<Vec<PatternMatcher>>,
    pub uri_relative_filter_groups: Option<Vec<UriRelativeFilterGroup>>,
    pub static_types: Option<Vec<String>>,
    /// The static and the dynamic types; a partial type (`image/*`) is
    /// kept as its base (`image`).
    pub types: Option<Vec<String>>,
    pub mime_groups: Option<Vec<String>>,
    pub has_static_partial_types: bool,
    pub has_dynamic_partial_types: bool,
    pub priority: i32,
    pub order: i32,
    pub auto_verify: bool,
    /// `VISIBILITY_*`.
    pub instant_app_visibility: i32,
    /// The extras an intent must carry, as their parcel
    /// (`PersistableBundle.writeToParcel`); empty when read from XML. No
    /// resolution passes an intent's extras, so such a filter never
    /// matches there.
    pub extras: Option<Vec<u8>>,
}

/// A MIME type the original refuses (`MalformedMimeTypeException`).
#[derive(Debug, PartialEq, Eq)]
pub struct MalformedMimeType;

/// `processMimeType`: the internal form of a type and whether it is
/// partial.
fn mime_type(ty: &str) -> std::result::Result<(String, bool), MalformedMimeType> {
    let t = utf16(ty);
    match t.iter().position(|&c| c == b'/' as u16) {
        Some(slash) if slash > 0 && t.len() >= slash + 2 => {
            if t.len() == slash + 2 && t[slash + 1] == b'*' as u16 {
                Ok((String::from_utf16_lossy(&t[..slash]), true))
            } else {
                Ok((ty.to_owned(), false))
            }
        }
        _ => Err(MalformedMimeType),
    }
}

fn push_unique(list: &mut Option<Vec<String>>, value: &str) {
    let list = list.get_or_insert_with(Vec::new);
    if !list.iter().any(|v| v == value) {
        list.push(value.to_owned());
    }
}

fn contains(list: &Option<Vec<String>>, value: &str) -> bool {
    list.as_ref().is_some_and(|l| l.iter().any(|v| v == value))
}

impl IntentFilter {
    pub fn add_action(&mut self, action: &str) {
        if !self.actions.iter().any(|a| a == action) {
            self.actions.push(action.to_owned());
        }
    }

    pub fn add_category(&mut self, category: &str) {
        push_unique(&mut self.categories, category);
    }

    pub fn add_data_scheme(&mut self, scheme: &str) {
        push_unique(&mut self.schemes, scheme);
    }

    pub fn add_mime_group(&mut self, group: &str) {
        push_unique(&mut self.mime_groups, group);
    }

    /// `addDataType`: a static type.
    pub fn add_data_type(&mut self, ty: &str) -> std::result::Result<(), MalformedMimeType> {
        let (internal, partial) = mime_type(ty)?;
        let static_types = self.static_types.get_or_insert_with(Vec::new);
        let types = self.types.get_or_insert_with(Vec::new);
        if !types.contains(&internal) {
            types.push(internal.clone());
            static_types.push(internal);
            self.has_static_partial_types |= partial;
        }
        Ok(())
    }

    /// `addDynamicDataType`: a type of a MIME group.
    pub fn add_dynamic_data_type(
        &mut self,
        ty: &str,
    ) -> std::result::Result<(), MalformedMimeType> {
        let (internal, partial) = mime_type(ty)?;
        let types = self.types.get_or_insert_with(Vec::new);
        if !types.contains(&internal) {
            types.push(internal);
            self.has_dynamic_partial_types |= partial;
        }
        Ok(())
    }

    /// `clearDynamicDataTypes`.
    pub fn clear_dynamic_data_types(&mut self) {
        if self.types.is_none() {
            return;
        }
        self.types = self.static_types.clone();
        self.has_dynamic_partial_types = false;
    }

    pub fn add_data_authority(&mut self, host: &str, port: Option<&str>) {
        let entry = AuthorityEntry::new(host, port);
        self.authorities.get_or_insert_with(Vec::new).push(entry);
    }

    pub fn add_data_path(&mut self, path: PatternMatcher) {
        self.paths.get_or_insert_with(Vec::new).push(path);
    }

    pub fn add_data_scheme_specific_part(&mut self, ssp: PatternMatcher) {
        self.ssps.get_or_insert_with(Vec::new).push(ssp);
    }

    pub fn add_uri_relative_filter_group(&mut self, group: UriRelativeFilterGroup) {
        let groups = self.uri_relative_filter_groups.get_or_insert_with(Vec::new);
        if !groups.contains(&group) {
            groups.push(group);
        }
    }

    pub fn has_action(&self, action: &str) -> bool {
        self.actions.iter().any(|a| a == action)
    }

    pub fn has_category(&self, category: &str) -> bool {
        contains(&self.categories, category)
    }

    pub fn has_data_scheme(&self, scheme: &str) -> bool {
        contains(&self.schemes, scheme)
    }

    pub fn count_data_authorities(&self) -> usize {
        self.authorities.as_ref().map_or(0, Vec::len)
    }

    pub fn is_visible_to_instant_app(&self) -> bool {
        self.instant_app_visibility != VISIBILITY_NONE
    }

    pub fn is_explicitly_visible_to_instant_app(&self) -> bool {
        self.instant_app_visibility == VISIBILITY_EXPLICIT
    }

    /// `handleAllWebDataURI`.
    pub fn handle_all_web_data_uri(&self) -> bool {
        self.has_category(CATEGORY_APP_BROWSER)
            || (self.handles_web_uris(false) && self.count_data_authorities() == 0)
    }

    /// `handlesWebUris`: a VIEW, BROWSABLE filter for http or https
    /// (only those, with `only_web_schemes`).
    pub fn handles_web_uris(&self, only_web_schemes: bool) -> bool {
        let Some(schemes) = &self.schemes else {
            return false;
        };
        if !self.has_action(ACTION_VIEW) || !self.has_category(CATEGORY_BROWSABLE) {
            return false;
        }
        if schemes.is_empty() {
            return false;
        }
        let web = |s: &String| s == "http" || s == "https";
        if only_web_schemes {
            schemes.iter().all(web)
        } else {
            schemes.iter().any(web)
        }
    }

    /// `matchAction`, with wildcard support and ignored actions.
    pub fn match_action(&self, action: &str, wildcards: bool, ignore: Option<&[&str]>) -> bool {
        if wildcards && action == WILDCARD {
            let Some(ignore) = ignore else {
                return !self.actions.is_empty();
            };
            return self.actions.iter().any(|a| !ignore.contains(&a.as_str()));
        }
        if ignore.is_some_and(|ignore| ignore.contains(&action)) {
            return false;
        }
        self.has_action(action)
    }

    /// `findMimeType`.
    fn find_mime_type(&self, ty: Option<&str>) -> bool {
        let (Some(types), Some(ty)) = (&self.types, ty) else {
            return false;
        };
        if types.iter().any(|t| t == ty) {
            return true;
        }
        // An intent wanting every type of the filter.
        if ty == "*/*" {
            return !types.is_empty();
        }
        let partial = self.has_static_partial_types || self.has_dynamic_partial_types;
        // A filter wanting every type.
        if partial && types.iter().any(|t| t == "*") {
            return true;
        }
        let t = utf16(ty);
        if let Some(slash) = t.iter().position(|&c| c == b'/' as u16).filter(|&s| s > 0) {
            let base = String::from_utf16_lossy(&t[..slash]);
            if partial && types.contains(&base) {
                return true;
            }
            if t.len() == slash + 2 && t[slash + 1] == b'*' as u16 {
                let prefix = &t[..=slash];
                return types.iter().any(|v| utf16(v).starts_with(prefix));
            }
        }
        false
    }

    /// `matchDataAuthority`.
    pub fn match_data_authority(&self, data: Option<&Uri>, wildcards: bool) -> i32 {
        let (Some(data), Some(authorities)) = (data, &self.authorities) else {
            return NO_MATCH_DATA;
        };
        authorities
            .iter()
            .map(|a| a.matches(data, wildcards))
            .find(|&m| m >= 0)
            .unwrap_or(NO_MATCH_DATA)
    }

    fn has_data_ssp(&self, ssp: Option<&str>, wildcards: bool) -> bool {
        let Some(ssps) = &self.ssps else { return false };
        if wildcards && ssp == Some(WILDCARD) && !ssps.is_empty() {
            return true;
        }
        ssps.iter().any(|p| p.matches(ssp))
    }

    fn has_data_path(&self, path: Option<&str>, wildcards: bool) -> bool {
        let Some(paths) = &self.paths else {
            return false;
        };
        if wildcards && path == Some(WILDCARD_PATH) {
            return true;
        }
        paths.iter().any(|p| p.matches(path))
    }

    /// `matchRelRefGroups`: the first group that matches decides.
    fn match_rel_ref_groups(&self, data: &Uri) -> bool {
        let groups = self
            .uri_relative_filter_groups
            .as_deref()
            .unwrap_or_default();
        UriRelativeFilterGroup::match_groups(groups, data)
    }

    /// `matchData(type, scheme, data, wildcardSupported)`.
    pub fn match_data(
        &self,
        ty: Option<&str>,
        scheme: Option<&str>,
        data: Option<&Uri>,
        wildcards: bool,
    ) -> i32 {
        let wildcard_with_mime_groups =
            wildcards && self.mime_groups.as_ref().is_some_and(|g| !g.is_empty());
        let mut m = MATCH_CATEGORY_EMPTY;
        if !wildcard_with_mime_groups && self.types.is_none() && self.schemes.is_none() {
            return if ty.is_none() && data.is_none() {
                MATCH_CATEGORY_EMPTY + MATCH_ADJUSTMENT_NORMAL
            } else {
                NO_MATCH_DATA
            };
        }
        if let Some(schemes) = &self.schemes {
            let scheme_or_empty = scheme.unwrap_or("");
            if schemes.iter().any(|s| s == scheme_or_empty) || wildcards && scheme == Some(WILDCARD)
            {
                m = MATCH_CATEGORY_SCHEME;
            } else {
                return NO_MATCH_DATA;
            }
            if let (Some(_), Some(data)) = (&self.ssps, data) {
                let ssp = data.scheme_specific_part();
                m = if self.has_data_ssp(ssp.as_deref(), wildcards) {
                    MATCH_CATEGORY_SCHEME_SPECIFIC_PART
                } else {
                    NO_MATCH_DATA
                };
            }
            if m != MATCH_CATEGORY_SCHEME_SPECIFIC_PART && self.authorities.is_some() {
                // Without a scheme-specific part, an authority must match.
                let auth = self.match_data_authority(data, wildcards);
                if auth < 0 {
                    return NO_MATCH_DATA;
                }
                if self.paths.is_none() && self.uri_relative_filter_groups.is_none() {
                    m = auth;
                } else {
                    let data = data.expect("an authority matched");
                    if self.has_data_path(data.path().as_deref(), wildcards)
                        || self.match_rel_ref_groups(data)
                    {
                        m = MATCH_CATEGORY_PATH;
                    } else {
                        return NO_MATCH_DATA;
                    }
                }
            }
            if m == NO_MATCH_DATA {
                return NO_MATCH_DATA;
            }
        } else if scheme.is_some_and(|s| {
            !s.is_empty() && s != "content" && s != "file" && !(wildcards && s == WILDCARD)
        }) {
            // A type-only filter matches no data, or content: and file:
            // data.
            return NO_MATCH_DATA;
        }
        if wildcard_with_mime_groups {
            return MATCH_CATEGORY_TYPE;
        } else if self.types.is_some() {
            if self.find_mime_type(ty) {
                m = MATCH_CATEGORY_TYPE;
            } else {
                return NO_MATCH_TYPE;
            }
        } else if ty.is_some() {
            // Without types, only an intent without one matches.
            return NO_MATCH_TYPE;
        }
        m + MATCH_ADJUSTMENT_NORMAL
    }

    /// `matchCategories`: whether every category of the intent is the
    /// filter's.
    pub fn match_categories(&self, categories: Option<&[String]>) -> bool {
        let Some(categories) = categories else {
            return true;
        };
        match &self.categories {
            None => categories.is_empty(),
            Some(mine) => categories.iter().all(|c| mine.contains(c)),
        }
    }

    /// `match(action, type, scheme, data, categories, logTag,
    /// supportWildcards, ignoreActions)`, with no extras.
    #[allow(clippy::too_many_arguments)]
    pub fn matches(
        &self,
        action: Option<&str>,
        ty: Option<&str>,
        scheme: Option<&str>,
        data: Option<&Uri>,
        categories: Option<&[String]>,
        wildcards: bool,
        ignore_actions: Option<&[&str]>,
    ) -> i32 {
        if let Some(action) = action
            && !self.match_action(action, wildcards, ignore_actions)
        {
            return NO_MATCH_ACTION;
        }
        let data_match = self.match_data(ty, scheme, data, wildcards);
        if data_match < 0 {
            return data_match;
        }
        if !self.match_categories(categories) {
            return NO_MATCH_CATEGORY;
        }
        if self.extras.is_some() {
            return NO_MATCH_EXTRAS;
        }
        data_match
    }

    /// `IntentFilter(Parcel)`.
    pub fn read(r: &mut Reader<'_>, strings: &mut dyn Strings) -> Result<IntentFilter> {
        let mut f = IntentFilter::default();
        for action in read_string_list(r, strings)? {
            f.add_action(&action);
        }
        let mut optional_list = |r: &mut Reader<'_>| -> Result<Option<Vec<String>>> {
            Ok(if r.read_i32()? != 0 {
                Some(read_string_list(r, strings)?)
            } else {
                None
            })
        };
        f.categories = optional_list(r)?;
        f.schemes = optional_list(r)?;
        f.static_types = optional_list(r)?;
        f.types = optional_list(r)?;
        f.mime_groups = optional_list(r)?;
        f.ssps = read_list(r, |r| PatternMatcher::read(r, strings))?;
        f.authorities = read_list(r, |r| AuthorityEntry::read(r, strings))?;
        f.paths = read_list(r, |r| PatternMatcher::read(r, strings))?;
        f.priority = r.read_i32()?;
        f.has_static_partial_types = r.read_i32()? > 0;
        f.has_dynamic_partial_types = r.read_i32()? > 0;
        f.auto_verify = r.read_i32()? > 0;
        f.instant_app_visibility = r.read_i32()?;
        f.order = r.read_i32()?;
        if r.read_i32()? != 0 {
            // A PersistableBundle: its length, then its magic, its data
            // and whether it has an intent.
            let start = r.position();
            let length = r.read_i32()?;
            if length > 0 {
                r.skip(4 + length as usize)?;
                r.read_bool()?;
            }
            f.extras = Some(r.since(start).0.to_vec());
        }
        f.uri_relative_filter_groups = read_list(r, |r| UriRelativeFilterGroup::read(r, strings))?;
        Ok(f)
    }

    /// `writeToParcel`.
    pub fn write(&self, p: &mut Parcel) {
        let strings = |p: &mut Parcel, list: &[String]| {
            p.write_i32(list.len() as i32);
            list.iter().for_each(|s| p.write_string16(Some(s)));
        };
        strings(p, &self.actions);
        for list in [
            &self.categories,
            &self.schemes,
            &self.static_types,
            &self.types,
            &self.mime_groups,
        ] {
            match list {
                Some(list) => {
                    p.write_i32(1);
                    strings(p, list);
                }
                None => p.write_i32(0),
            }
        }
        let patterns = |p: &mut Parcel, list: &Option<Vec<PatternMatcher>>| {
            let list = list.as_deref().unwrap_or_default();
            p.write_i32(list.len() as i32);
            list.iter().for_each(|m| m.write(p));
        };
        patterns(p, &self.ssps);
        let authorities = self.authorities.as_deref().unwrap_or_default();
        p.write_i32(authorities.len() as i32);
        authorities.iter().for_each(|a| a.write(p));
        patterns(p, &self.paths);
        p.write_i32(self.priority);
        p.write_i32(self.has_static_partial_types as i32);
        p.write_i32(self.has_dynamic_partial_types as i32);
        p.write_i32(self.auto_verify as i32);
        p.write_i32(self.instant_app_visibility);
        p.write_i32(self.order);
        match &self.extras {
            Some(extras) => {
                p.write_i32(1);
                if extras.is_empty() {
                    p.write_i32(0);
                } else {
                    p.write_raw(extras, &[]);
                }
            }
            None => p.write_i32(0),
        }
        let groups = self
            .uri_relative_filter_groups
            .as_deref()
            .unwrap_or_default();
        p.write_i32(groups.len() as i32);
        groups.iter().for_each(|g| g.write(p));
    }

    /// `readFromXml`, of the element holding the filter (a preferred
    /// activity's `<filter>`).
    pub fn parse(e: &Element) -> IntentFilter {
        let mut f = IntentFilter::default();
        // The original reads `autoVerify` with `Boolean.getBoolean`, a
        // system property lookup, so it is always false.
        for c in e.children() {
            let name = c.string("name");
            let pattern = || {
                [
                    ("literal", PATTERN_LITERAL),
                    ("prefix", PATTERN_PREFIX),
                    ("sglob", PATTERN_SIMPLE_GLOB),
                    ("aglob", PATTERN_ADVANCED_GLOB),
                    ("suffix", PATTERN_SUFFIX),
                ]
                .into_iter()
                .find_map(|(attr, kind)| PatternMatcher::new(&c.string(attr)?, kind).ok())
            };
            match (c.name.as_str(), name) {
                ("action", Some(n)) => f.add_action(&n),
                ("cat", Some(n)) => f.add_category(&n),
                ("staticType", Some(n)) => f.add_data_type(&n).unwrap_or(()),
                ("type", Some(n)) => f.add_dynamic_data_type(&n).unwrap_or(()),
                ("group", Some(n)) => f.add_mime_group(&n),
                ("scheme", Some(n)) => f.add_data_scheme(&n),
                ("ssp", _) => {
                    if let Some(p) = pattern() {
                        f.add_data_scheme_specific_part(p);
                    }
                }
                ("auth", _) => {
                    if let Some(host) = c.string("host") {
                        f.add_data_authority(&host, c.string("port").as_deref());
                    }
                }
                ("path", _) => {
                    if let Some(p) = pattern() {
                        f.add_data_path(p);
                    }
                }
                ("extras", _) => f.extras = Some(Vec::new()),
                ("uriRelativeFilterGroup", _) => {
                    f.add_uri_relative_filter_group(UriRelativeFilterGroup::parse(c))
                }
                _ => {}
            }
        }
        f
    }
}

/// A list the original writes as its size and its elements, null when
/// empty.
fn read_list<T>(
    r: &mut Reader<'_>,
    mut read: impl FnMut(&mut Reader<'_>) -> Result<T>,
) -> Result<Option<Vec<T>>> {
    let n = r.read_i32()?;
    if n <= 0 {
        return Ok(None);
    }
    (0..n).map(|_| read(r)).collect::<Result<_>>().map(Some)
}

/// `ParsedIntentInfo`: a component's filter and what a match shows of it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ParsedIntentInfo {
    pub filter: IntentFilter,
    pub has_default: bool,
    pub label_res: i32,
    pub non_localized_label: Option<String>,
    pub icon: i32,
}

impl ParsedIntentInfo {
    /// `ParsedIntentInfoImpl(Parcel)`.
    pub fn read(r: &mut Reader<'_>, strings: &mut dyn Strings) -> Result<ParsedIntentInfo> {
        let flags = r.read_i32()? & 0xff;
        let label_res = r.read_i32()?;
        let non_localized_label = if flags & 0x4 != 0 {
            read_char_sequence(r, strings)?
        } else {
            None
        };
        let icon = r.read_i32()?;
        let filter = if r.read_i32()? != 0 {
            IntentFilter::read(r, strings)?
        } else {
            // The original never writes a null filter.
            return Err(BAD_VALUE);
        };
        Ok(ParsedIntentInfo {
            filter,
            has_default: flags & 0x1 != 0,
            label_res,
            non_localized_label,
            icon,
        })
    }
}

/// `Parcel.readCharSequence` (`TextUtils.CHAR_SEQUENCE_CREATOR`) of a
/// plain string; a styled one (`Spanned`, whose spans follow) is refused.
pub fn read_char_sequence(r: &mut Reader<'_>, strings: &mut dyn Strings) -> Result<Option<String>> {
    let kind = r.read_i32()?;
    let Some(s) = strings.string8(r)? else {
        return Ok(None);
    };
    if kind != 1 {
        return Err(BAD_VALUE);
    }
    Ok(Some(s))
}

#[cfg(test)]
mod tests;
