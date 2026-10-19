//! `android.net.Uri` as intent resolution reads it: the parts of a URI
//! string, parsed as `Uri.StringUri`, `OpaqueUri` and `HierarchicalUri`
//! parse it, and decoded as `UriCodec.decode` decodes them (sources at
//! `android-16.0.0_r1`, `frameworks/base/core/java/android/net`). A
//! parcelled `Uri` is its type and its string, whichever class wrote it.

use aim_binder_host::parcel::{BAD_VALUE, Reader, Result};

use super::intent_filter::Strings;

/// A URI and the class it was parcelled as.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Uri {
    kind: Kind,
    string: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// `Uri.parse`'s: every part is parsed from the string.
    String,
    /// `Uri.fromParts`: a scheme, a scheme-specific part and a fragment.
    Opaque,
    /// `Uri.Builder`'s: the parts of a hierarchical URI, the path made
    /// absolute when there is a scheme or an authority.
    Hierarchical,
    /// Native Uri.Builder defaults retain their known empty path.
    PreferredHierarchical,
}

/// `Uri.*.TYPE_ID`.
const NULL_TYPE_ID: i32 = 0;
const STRING_TYPE_ID: i32 = 1;
const OPAQUE_TYPE_ID: i32 = 2;
const HIERARCHICAL_TYPE_ID: i32 = 3;

impl Uri {
    /// Decoded Uri.Builder parts used by Settings' preferred-app expansion.
    pub fn preferred_hierarchical(scheme: &str, authority: Option<&str>, path: Option<&str>) -> Uri {
        let mut string = format!("{scheme}:");
        if let Some(authority) = authority { string.push_str("//"); string.push_str(&preferred_encode(authority, "@:")); }
        if let Some(path) = path {
            if !path.is_empty() && !path.starts_with('/') { string.push('/'); }
            string.push_str(&preferred_encode(path, "/"));
        }
        Uri { kind: Kind::PreferredHierarchical, string }
    }
    pub fn preferred_opaque(scheme: &str, part: &str) -> Uri {
        Uri { kind: Kind::Opaque, string: format!("{scheme}:{}", preferred_encode(part, "")) }
    }

    /// `Uri.parse`.
    pub fn parse(string: &str) -> Uri {
        Uri {
            kind: Kind::String,
            string: string.to_owned(),
        }
    }

    /// `Uri.CREATOR.createFromParcel`; `None` for `Uri.NULL_TYPE_ID`.
    pub fn read(r: &mut Reader<'_>, strings: &mut dyn Strings) -> Result<Option<Uri>> {
        let kind = match r.read_i32()? {
            NULL_TYPE_ID => return Ok(None),
            STRING_TYPE_ID => Kind::String,
            OPAQUE_TYPE_ID => Kind::Opaque,
            HIERARCHICAL_TYPE_ID => Kind::Hierarchical,
            _ => return Err(BAD_VALUE),
        };
        let string = strings.string8(r)?.ok_or(BAD_VALUE)?;
        Ok(Some(Uri { kind, string }))
    }

    pub(crate) fn write_cache(&self, writer: &mut super::parse::parcel::Writer) {
        writer.int(match self.kind {
            Kind::String => STRING_TYPE_ID,
            Kind::Opaque => OPAQUE_TYPE_ID,
            Kind::Hierarchical | Kind::PreferredHierarchical => HIERARCHICAL_TYPE_ID,
        });
        writer.string(Some(&self.string));
    }

    /// The class of the `Uri`, as `Parcel.writeParcelable` names it.
    pub fn class_name(&self) -> &'static str {
        match self.kind {
            Kind::String => "android.net.Uri$StringUri",
            Kind::Opaque => "android.net.Uri$OpaqueUri",
            Kind::Hierarchical | Kind::PreferredHierarchical => "android.net.Uri$HierarchicalUri",
        }
    }

    /// `Uri.writeToParcel`: its type, then its string.
    pub fn write(&self, p: &mut aim_binder_host::parcel::Parcel) {
        p.write_i32(match self.kind {
            Kind::String => STRING_TYPE_ID,
            Kind::Opaque => OPAQUE_TYPE_ID,
            Kind::Hierarchical | Kind::PreferredHierarchical => HIERARCHICAL_TYPE_ID,
        });
        p.write_string8(Some(&self.string));
    }

    /// The string `toString` returns for a URI read from a parcel.
    pub fn as_str(&self) -> &str {
        &self.string
    }

    /// `findSchemeSeparator`.
    fn ssi(&self) -> Option<usize> {
        self.string.find(':')
    }

    /// `findFragmentSeparator`: the first `#` from the scheme separator on.
    fn fsi(&self) -> Option<usize> {
        let from = self.ssi().unwrap_or(0);
        self.string[from..].find('#').map(|i| from + i)
    }

    pub fn scheme(&self) -> Option<&str> {
        self.ssi().map(|ssi| &self.string[..ssi])
    }

    /// The scheme-specific part, decoded.
    pub fn scheme_specific_part(&self) -> Option<String> {
        let encoded = match self.kind {
            Kind::String | Kind::Opaque => self.encoded_ssp(),
            Kind::Hierarchical | Kind::PreferredHierarchical => {
                let mut ssp = String::new();
                if let Some(authority) = self.encoded_authority() {
                    ssp.push_str("//");
                    ssp.push_str(authority);
                }
                if let Some(path) = self.encoded_path() {
                    ssp.push_str(&path);
                }
                if let Some(query) = self.encoded_query().filter(|q| !q.is_empty()) {
                    ssp.push('?');
                    ssp.push_str(query);
                }
                ssp
            }
        };
        Some(decode(&encoded))
    }

    /// `StringUri.parseSsp`.
    fn encoded_ssp(&self) -> String {
        let start = self.ssi().map_or(0, |ssi| ssi + 1);
        match self.fsi() {
            Some(fsi) if fsi >= start => self.string[start..fsi].to_owned(),
            Some(_) => String::new(),
            None => self.string[start..].to_owned(),
        }
    }

    /// `StringUri.parseAuthority`: what follows `//` after the scheme
    /// separator, up to a path, query or fragment.
    fn encoded_authority(&self) -> Option<&str> {
        if self.kind == Kind::Opaque {
            return None;
        }
        let s = self.string.as_bytes();
        let start = self.ssi().map_or(0, |ssi| ssi + 1);
        if s.len() > start + 1 && s[start] == b'/' && s[start + 1] == b'/' {
            let from = start + 2;
            let end = s[from..]
                .iter()
                .position(|c| matches!(c, b'/' | b'\\' | b'?' | b'#'))
                .map_or(s.len(), |i| from + i);
            Some(&self.string[from..end])
        } else {
            None
        }
    }

    /// The decoded authority.
    pub fn authority(&self) -> Option<String> {
        self.encoded_authority().map(decode)
    }

    /// `AbstractHierarchicalUri.parseHost`.
    pub fn host(&self) -> Option<String> {
        let authority = self.encoded_authority()?;
        let user_info_end = authority.rfind('@').map_or(0, |i| i + 1);
        let host = match port_separator(authority) {
            Some(port) if port >= user_info_end => &authority[user_info_end..port],
            Some(_) => "",
            None => &authority[user_info_end..],
        };
        Some(decode(host))
    }

    /// `AbstractHierarchicalUri.parsePort`: -1 without a port or when it
    /// is not a number.
    pub fn port(&self) -> i32 {
        let Some(authority) = self.encoded_authority() else {
            return -1;
        };
        let Some(sep) = port_separator(authority) else {
            return -1;
        };
        parse_int(&decode(&authority[sep + 1..])).unwrap_or(-1)
    }

    /// `StringUri.parsePath`, made absolute for a `HierarchicalUri`.
    fn encoded_path(&self) -> Option<String> {
        if self.kind == Kind::Opaque {
            return None;
        }
        let s = self.string.as_bytes();
        let ssi = self.ssi();
        let mut start = ssi.map_or(0, |ssi| ssi + 1);
        if self.kind == Kind::PreferredHierarchical && start == s.len() { return Some(String::new()); }
        if ssi.is_some() && (start == s.len() || s[start] != b'/') {
            return None;
        }
        if s.len() > start + 1 && s[start] == b'/' && s[start + 1] == b'/' {
            start += 2;
            loop {
                match s.get(start) {
                    None | Some(b'/' | b'\\') => break,
                    Some(b'?' | b'#') => return Some(self.absolute(String::new())),
                    Some(_) => start += 1,
                }
            }
        }
        let end = s[start..]
            .iter()
            .position(|c| matches!(c, b'?' | b'#'))
            .map_or(s.len(), |i| start + i);
        Some(self.absolute(self.string[start..end].to_owned()))
    }

    /// `HierarchicalUri.generatePath`: a path without a leading `/` gets
    /// one when the URI has a scheme or an authority.
    fn absolute(&self, path: String) -> String {
        let has_scheme_or_authority = self.scheme().is_some_and(|s| !s.is_empty())
            || self.encoded_authority().is_some_and(|a| !a.is_empty());
        if matches!(self.kind, Kind::Hierarchical | Kind::PreferredHierarchical)
            && has_scheme_or_authority
            && !path.is_empty()
            && !path.starts_with('/')
        {
            format!("/{path}")
        } else {
            path
        }
    }

    /// The decoded path.
    pub fn path(&self) -> Option<String> {
        self.encoded_path().map(|p| decode(&p))
    }

    /// `getPathSegments`: the decoded non-empty segments of the path.
    pub fn path_segments(&self) -> Vec<String> {
        self.encoded_path()
            .map(|p| p.split('/').filter(|s| !s.is_empty()).map(decode).collect())
            .unwrap_or_default()
    }

    /// `StringUri.parseQuery`.
    fn encoded_query(&self) -> Option<&str> {
        if self.kind == Kind::Opaque {
            return None;
        }
        let from = self.ssi().unwrap_or(0);
        let qsi = from + self.string[from..].find('?')?;
        match self.fsi() {
            None => Some(&self.string[qsi + 1..]),
            Some(fsi) if fsi < qsi => None,
            Some(fsi) => Some(&self.string[qsi + 1..fsi]),
        }
    }

    /// The decoded query.
    pub fn query(&self) -> Option<String> {
        self.encoded_query().map(decode)
    }

    /// The decoded fragment.
    pub fn fragment(&self) -> Option<String> {
        self.fsi().map(|fsi| decode(&self.string[fsi + 1..]))
    }

    /// `isHierarchical`.
    fn is_hierarchical(&self) -> bool {
        match self.kind {
            Kind::Opaque => false,
            Kind::Hierarchical | Kind::PreferredHierarchical => true,
            Kind::String => self.ssi().is_none_or(|ssi| self.string.as_bytes().get(ssi + 1) == Some(&b'/')),
        }
    }

    /// `getUserInfo`: the decoded part of the authority before its last
    /// `@`.
    pub fn user_info(&self) -> Option<String> {
        let authority = self.encoded_authority()?;
        authority.rfind('@').map(|end| decode(&authority[..end]))
    }

    /// `buildUpon().encodedAuthority(authority).build()`: a hierarchical
    /// URI with its authority replaced (none for null), its path made
    /// absolute and an empty query or fragment left out, as
    /// `HierarchicalUri` writes it; an opaque one as `OpaqueUri` writes it.
    pub fn with_encoded_authority(&self, authority: Option<&str>) -> Uri {
        let mut string = self.scheme().map(|s| format!("{s}:")).unwrap_or_default();
        let fragment = self.fsi().map(|fsi| &self.string[fsi + 1..]).filter(|f| !f.is_empty());
        if !self.is_hierarchical() {
            string.push_str(&self.encoded_ssp());
            if let Some(fragment) = fragment {
                string.push('#');
                string.push_str(fragment);
            }
            return Uri { kind: Kind::Opaque, string };
        }
        if let Some(authority) = authority {
            string.push_str("//");
            string.push_str(authority);
        }
        let path = self.encoded_path().unwrap_or_default();
        if !path.is_empty() && !path.starts_with('/') && (self.scheme().is_some() || authority.is_some()) {
            string.push('/');
        }
        string.push_str(&path);
        if let Some(query) = self.encoded_query().filter(|q| !q.is_empty()) {
            string.push('?');
            string.push_str(query);
        }
        if let Some(fragment) = fragment {
            string.push('#');
            string.push_str(fragment);
        }
        Uri { kind: Kind::Hierarchical, string }
    }

    /// `Uri.withAppendedPath` of a hierarchical URI with no query or
    /// fragment: `segment` appended to its path as it is
    /// (`appendEncodedPath`).
    pub fn with_appended_path(&self, segment: &str) -> Uri {
        let mut string = self.with_encoded_authority(self.encoded_authority()).string;
        if !string.ends_with('/') {
            string.push('/');
        }
        string.push_str(segment);
        Uri { kind: Kind::Hierarchical, string }
    }
}

/// `Uri.encode`: what is not an unreserved character escaped.
pub fn encode(value: &str) -> String {
    preferred_encode(value, "")
}

/// `Uri.encode(value, allow)`: what is neither unreserved nor in `allow`
/// escaped.
pub fn encode_allowing(value: &str, allow: &str) -> String {
    preferred_encode(value, allow)
}

/// `findPortSeparator`: the last `:` of the authority when only digits
/// follow it.
fn port_separator(authority: &str) -> Option<usize> {
    for (i, c) in authority.bytes().enumerate().rev() {
        if c == b':' {
            return Some(i);
        }
        if !c.is_ascii_digit() {
            return None;
        }
    }
    None
}

/// `Integer.parseInt`.
pub fn parse_int(s: &str) -> Option<i32> {
    let digits = s.strip_prefix(['-', '+']).unwrap_or(s);
    if digits.is_empty() || !digits.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// `Uri.decode`: `UriCodec.decode(s, false, UTF_8, false)`. An invalid
/// escape becomes U+FFFD; as in the original, a `%` followed by a
/// non-hex digit still adds the (partial) byte it was decoding, and an
/// escape cut off by the end of the string ends the decoding.
pub fn decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut bytes: Vec<u8> = Vec::new();
    let flush = |out: &mut String, bytes: &mut Vec<u8>| {
        if !bytes.is_empty() {
            out.push_str(&String::from_utf8_lossy(bytes));
            bytes.clear();
        }
    };
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            flush(&mut out, &mut bytes);
            out.push(c);
            continue;
        }
        let mut value: u8 = 0;
        for _ in 0..2 {
            let Some(c) = chars.next() else {
                flush(&mut out, &mut bytes);
                out.push('\u{fffd}');
                return out;
            };
            match c.to_digit(16) {
                Some(d) => value = value.wrapping_mul(16).wrapping_add(d as u8),
                None => {
                    flush(&mut out, &mut bytes);
                    out.push('\u{fffd}');
                    break;
                }
            }
        }
        bytes.push(value);
    }
    flush(&mut out, &mut bytes);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_hierarchical_uri() {
        let uri = Uri::parse("https://user@Example.com:8080/a%20b/c?x=1&y=2#frag");
        assert_eq!(uri.scheme(), Some("https"));
        assert_eq!(uri.authority().as_deref(), Some("user@Example.com:8080"));
        assert_eq!(uri.host().as_deref(), Some("Example.com"));
        assert_eq!(uri.port(), 8080);
        assert_eq!(uri.path().as_deref(), Some("/a b/c"));
        assert_eq!(uri.query().as_deref(), Some("x=1&y=2"));
        assert_eq!(uri.fragment().as_deref(), Some("frag"));
        assert_eq!(
            uri.scheme_specific_part().as_deref(),
            Some("//user@Example.com:8080/a b/c?x=1&y=2")
        );
    }

    #[test]
    fn parses_an_opaque_uri() {
        let uri = Uri::parse("mailto:someone@example.com#x");
        assert_eq!(uri.scheme(), Some("mailto"));
        assert_eq!(uri.host(), None);
        assert_eq!(uri.port(), -1);
        assert_eq!(uri.path(), None);
        assert_eq!(uri.query(), None);
        assert_eq!(
            uri.scheme_specific_part().as_deref(),
            Some("someone@example.com")
        );
    }

    #[test]
    fn parses_edge_cases() {
        // A scheme only, a relative URI, an authority with no path.
        let uri = Uri::parse("scheme1:");
        assert_eq!((uri.scheme(), uri.path()), (Some("scheme1"), None));
        assert_eq!(uri.scheme_specific_part().as_deref(), Some(""));
        let uri = Uri::parse("a/b?c");
        assert_eq!(uri.scheme(), None);
        assert_eq!(uri.path().as_deref(), Some("a/b"));
        let uri = Uri::parse("http://host?q");
        assert_eq!(uri.host().as_deref(), Some("host"));
        assert_eq!(uri.path().as_deref(), Some(""));
        let uri = Uri::parse("http://host:abc/");
        assert_eq!((uri.host().as_deref(), uri.port()), (Some("host:abc"), -1));
        let uri = Uri::parse("scheme1://");
        assert_eq!(uri.host().as_deref(), Some(""));
        let uri = Uri::parse("http://h\\p");
        assert_eq!(uri.path().as_deref(), Some("\\p"));
    }

    #[test]
    fn makes_a_builders_path_absolute() {
        let mut r = Reader::new(&[], &[]);
        assert!(Uri::read(&mut r, &mut super::super::intent_filter::Plain).is_err());
        let uri = Uri {
            kind: Kind::Hierarchical,
            string: "content://auth".into(),
        };
        assert_eq!(uri.path().as_deref(), Some(""));
        let uri = Uri {
            kind: Kind::Opaque,
            string: "tel:123#f".into(),
        };
        assert_eq!(uri.host(), None);
        assert_eq!(uri.fragment().as_deref(), Some("f"));
    }

    #[test]
    fn decodes_as_uri_codec() {
        assert_eq!(decode("a%41b"), "aAb");
        assert_eq!(decode("%E2%82%AC"), "\u{20ac}");
        assert_eq!(decode("+"), "+");
        assert_eq!(decode("%zz"), "\u{fffd}\0z");
        assert_eq!(decode("ab%4"), "ab\u{fffd}");
        assert_eq!(decode("%FF"), "\u{fffd}");
    }

    #[test]
    fn builds_upon_a_uri_with_another_authority() {
        let uri = Uri::parse("content://10@a.b/x%20y?q#f");
        assert_eq!(uri.user_info().as_deref(), Some("10"));
        let built = uri.with_encoded_authority(Some(&encode("a.b")));
        assert_eq!((built.as_str(), built.class_name()), ("content://a.b/x%20y?q#f", "android.net.Uri$HierarchicalUri"));
        assert_eq!(Uri::parse("content://a.b?#").with_encoded_authority(Some("a.b")).as_str(), "content://a.b");
        assert_eq!(Uri::parse("content://a:1/x").with_encoded_authority(Some(&encode("a:1"))).as_str(), "content://a%3A1/x");
        assert_eq!(Uri::parse("content:x#").with_encoded_authority(Some("y")).as_str(), "content:x");
        assert_eq!(Uri::parse("content://a.b/x").user_info(), None);
    }

    /// `Settings.Global.CONTENT_URI` with a name appended, encoded as it is.
    #[test]
    fn appends_a_path() {
        let uri = Uri::parse("content://settings/global").with_appended_path("a b");
        assert_eq!((uri.as_str(), uri.class_name()), ("content://settings/global/a b", "android.net.Uri$HierarchicalUri"));
        assert_eq!(Uri::parse("content://settings/").with_appended_path("x").as_str(), "content://settings/x");
    }
}


fn preferred_encode(value: &str, allowed: &str) -> String {
    let mut encoded = String::new();
    for &byte in value.as_bytes() {
        let character = char::from(byte);
        if byte.is_ascii_alphanumeric() || "_-!.~'()*".contains(character) || allowed.contains(character) { encoded.push(character); }
        else { encoded.push('%'); encoded.push_str(&format!("{byte:02X}")); }
    }
    encoded
}
