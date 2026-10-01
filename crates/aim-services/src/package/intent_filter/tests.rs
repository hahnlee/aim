//! Matching, checked with the cases of CTS's
//! `android.content.cts.IntentFilterTest` and `android.os.cts.PatternMatcherTest`
//! and of frameworks/base's `android.os.PatternMatcherTest` at
//! `android-16.0.0_r1` (Copyright The Android Open Source Project, Apache
//! License 2.0), in a Rust form of their `Match`/`checkMatches` helpers.
//! Like `checkMatches`, a case compares the match category only.

use aim_binder_host::parcel::{Parcel, Reader};

use super::*;

const ACTION: &str = "testAction";
const CATEGORY: &str = "testCategory";
const DATA_STATIC_TYPE: &str = "vnd.android.cursor.dir/person";
const DATA_DYNAMIC_TYPE: &str = "type/dynamic";
const DATA_SCHEME: &str = "testDataSchemes.";
const HOST: &str = "testHost";
const PORT: &str = "80";
const DATA_PATH: &str = "testDataPath";
const URI: &str = "content://com.example/people";

type Strs<'a> = Option<&'a [Option<&'a str>]>;

fn each<'a>(s: Strs<'a>) -> impl Iterator<Item = &'a str> {
    s.into_iter().flatten().map(|s| s.expect("a string"))
}

/// CTS's `Match`: a filter built from its parts.
struct Match(IntentFilter);

impl Match {
    fn new(
        actions: Strs,
        categories: Strs,
        types: Strs,
        schemes: Strs,
        authorities: Strs,
        ports: Strs,
    ) -> Match {
        Match(IntentFilter::default())
            .actions(actions)
            .types(types)
            .with(|f| {
                each(categories).for_each(|c| f.add_category(c));
                each(schemes).for_each(|s| f.add_data_scheme(s));
                for (i, a) in each(authorities).enumerate() {
                    f.add_data_authority(a, ports.and_then(|p| p[i]));
                }
            })
    }

    fn with(mut self, f: impl FnOnce(&mut IntentFilter)) -> Match {
        f(&mut self.0);
        self
    }

    fn paths(self, paths: Strs, kinds: Option<&[i32]>) -> Match {
        self.with(|f| {
            for (i, p) in each(paths).enumerate() {
                f.add_data_path(PatternMatcher::new(p, kinds.unwrap()[i]).unwrap());
            }
        })
    }

    fn ssps(self, ssps: Strs, kinds: Option<&[i32]>) -> Match {
        self.with(|f| {
            for (i, p) in each(ssps).enumerate() {
                f.add_data_scheme_specific_part(PatternMatcher::new(p, kinds.unwrap()[i]).unwrap());
            }
        })
    }

    fn actions(self, actions: Strs) -> Match {
        self.with(|f| each(actions).for_each(|a| f.add_action(a)))
    }

    fn types(self, types: Strs) -> Match {
        self.with(|f| each(types).for_each(|t| f.add_data_type(t).unwrap()))
    }

    fn dynamic_types(self, types: Strs) -> Match {
        self.with(|f| each(types).for_each(|t| f.add_dynamic_data_type(t).unwrap()))
    }

    fn groups(self, groups: Strs) -> Match {
        self.with(|f| each(groups).for_each(|g| f.add_mime_group(g)))
    }
}

fn scheme_and_ssp(scheme: &str, ssp: &str, kind: i32) -> Match {
    Match::new(None, None, None, Some(&[Some(scheme)]), None, None)
        .ssps(Some(&[Some(ssp)]), Some(&[kind]))
}

/// CTS's `MatchCondition`.
struct Cond<'a> {
    result: i32,
    action: Option<&'a str>,
    categories: Strs<'a>,
    mime: Option<&'a str>,
    data: Option<&'a str>,
    wildcards: bool,
    ignored: Option<&'a [&'a str]>,
}

fn mc<'a>(
    result: i32,
    action: Option<&'a str>,
    categories: Strs<'a>,
    mime: Option<&'a str>,
    data: Option<&'a str>,
    wildcards: bool,
    ignored: Option<&'a [&'a str]>,
) -> Cond<'a> {
    Cond {
        result,
        action,
        categories,
        mime,
        data,
        wildcards,
        ignored,
    }
}

fn check(filter: &Match, conds: &[Cond]) {
    for c in conds {
        let categories: Vec<String> = each(c.categories).map(str::to_owned).collect();
        let categories = (!categories.is_empty()).then_some(categories);
        let uri = c.data.map(Uri::parse);
        let scheme = uri.as_ref().and_then(Uri::scheme);
        let result = filter.0.matches(
            c.action,
            c.mime,
            scheme,
            uri.as_ref(),
            categories.as_deref(),
            c.wildcards,
            c.ignored,
        );
        assert_eq!(
            result & MATCH_CATEGORY_MASK,
            c.result & MATCH_CATEGORY_MASK,
            "action {:?} type {:?} data {:?} categories {categories:?} wildcards {}: got {result:#x}, \
             expected {:#x}\n{:#?}",
            c.action,
            c.mime,
            c.data,
            c.wildcards,
            c.result,
            filter.0,
        );
    }
}

fn check_all(filters: &[Match], conds: &[Cond]) {
    filters.iter().for_each(|f| check(f, conds));
}

fn match_data(f: &IntentFilter, ty: Option<&str>, scheme: Option<&str>, data: Option<&str>) -> i32 {
    f.match_data(ty, scheme, data.map(Uri::parse).as_ref(), false)
}

/// CTS `testMatchData`.
#[test]
fn match_data_steps() {
    let mut f = IntentFilter::default();
    let empty = MATCH_CATEGORY_EMPTY + MATCH_ADJUSTMENT_NORMAL;
    assert_eq!(match_data(&f, None, None, None), empty);
    assert_eq!(match_data(&f, None, Some(DATA_SCHEME), None), empty);
    assert_eq!(
        match_data(&f, None, Some(DATA_SCHEME), Some(URI)),
        NO_MATCH_DATA
    );
    assert_eq!(
        match_data(&f, Some(DATA_STATIC_TYPE), Some(DATA_SCHEME), Some(URI)),
        NO_MATCH_DATA
    );
    f.add_data_scheme(DATA_SCHEME);
    assert_eq!(
        match_data(
            &f,
            Some(DATA_STATIC_TYPE),
            Some("mDataSchemestest"),
            Some(URI)
        ),
        NO_MATCH_DATA
    );
    assert_eq!(
        match_data(&f, Some(DATA_STATIC_TYPE), Some(""), Some(URI)),
        NO_MATCH_DATA
    );
    assert_eq!(
        match_data(&f, None, Some(DATA_SCHEME), Some(URI)),
        MATCH_CATEGORY_SCHEME + MATCH_ADJUSTMENT_NORMAL
    );
    assert_eq!(
        match_data(&f, Some(DATA_STATIC_TYPE), Some(DATA_SCHEME), Some(URI)),
        NO_MATCH_TYPE
    );
    f.add_data_type(DATA_STATIC_TYPE).unwrap();
    assert_eq!(
        match_data(&f, Some(DATA_STATIC_TYPE), Some(DATA_SCHEME), Some(URI)),
        MATCH_CATEGORY_TYPE + MATCH_ADJUSTMENT_NORMAL
    );
    f.add_data_authority(HOST, Some(PORT));
    assert_eq!(
        match_data(&f, None, Some(DATA_SCHEME), Some(URI)),
        NO_MATCH_DATA
    );
    f.add_data_path(PatternMatcher::new(DATA_PATH, PATTERN_LITERAL).unwrap());
    let uri = format!("http://{HOST}:{PORT}");
    assert_eq!(
        match_data(&f, None, Some(DATA_SCHEME), Some(&uri)),
        NO_MATCH_DATA
    );
}

/// CTS `testMatchDataWithRelRefGroups`.
#[test]
fn match_data_with_rel_ref_groups() {
    let uri = format!("https://{HOST}/path?query=string&cat=gizmo#fragment");
    let uri2 = format!("https://{HOST}/path?query=string;cat=gizmo#fragment");
    let mut f = IntentFilter::default();
    f.add_data_scheme(DATA_SCHEME);
    f.add_data_authority(HOST, None);
    let at = |f: &IntentFilter, uri: &str| match_data(f, None, Some(DATA_SCHEME), Some(uri));
    let path = MATCH_CATEGORY_PATH + MATCH_ADJUSTMENT_NORMAL;
    assert_eq!(at(&f, &uri), MATCH_CATEGORY_HOST + MATCH_ADJUSTMENT_NORMAL);
    let group = |action, filters: &[(i32, i32, &str)]| {
        let mut g = UriRelativeFilterGroup::new(action);
        filters
            .iter()
            .for_each(|&(part, kind, filter)| g.add(part, kind, filter));
        g
    };
    let with = |groups: &[&UriRelativeFilterGroup]| {
        let mut f = f.clone();
        groups
            .iter()
            .for_each(|&g| f.add_uri_relative_filter_group(g.clone()));
        f
    };
    let match_group = group(ACTION_ALLOW, &[(URI_PART_PATH, PATTERN_LITERAL, "/path")]);
    assert_eq!(at(&with(&[&match_group]), &uri), path);
    let no_match_group = group(ACTION_ALLOW, &[(URI_PART_PATH, PATTERN_LITERAL, "/trail")]);
    assert_eq!(at(&with(&[&no_match_group]), &uri), NO_MATCH_DATA);
    let match_group1 = group(
        ACTION_ALLOW,
        &[
            (URI_PART_PATH, PATTERN_LITERAL, "/path"),
            (URI_PART_QUERY, PATTERN_SIMPLE_GLOB, ".*"),
        ],
    );
    assert_eq!(at(&with(&[&match_group1]), &uri), path);
    assert_eq!(at(&with(&[&match_group1]), &uri2), path);
    let no_match_group1 = group(
        ACTION_ALLOW,
        &[
            (URI_PART_PATH, PATTERN_LITERAL, "/path"),
            (URI_PART_QUERY, PATTERN_PREFIX, "query"),
            (URI_PART_QUERY, PATTERN_SUFFIX, "widget"),
        ],
    );
    assert_eq!(at(&with(&[&no_match_group1]), &uri), NO_MATCH_DATA);
    assert_eq!(at(&with(&[&no_match_group1]), &uri2), NO_MATCH_DATA);
    let match_group2 = group(
        ACTION_ALLOW,
        &[
            (URI_PART_PATH, PATTERN_LITERAL, "/path"),
            (URI_PART_QUERY, PATTERN_PREFIX, "query"),
            (URI_PART_FRAGMENT, PATTERN_SIMPLE_GLOB, "fr.*"),
        ],
    );
    assert_eq!(at(&with(&[&match_group2]), &uri), path);
    assert_eq!(at(&with(&[&match_group2]), &uri2), path);
    let disallow = group(
        1,
        &[
            (URI_PART_PATH, PATTERN_LITERAL, "/path"),
            (URI_PART_QUERY, PATTERN_PREFIX, "query"),
            (URI_PART_QUERY, PATTERN_SUFFIX, "gizmo"),
            (URI_PART_FRAGMENT, PATTERN_SIMPLE_GLOB, "fr.*"),
        ],
    );
    assert_eq!(at(&with(&[&disallow]), &uri), NO_MATCH_DATA);
    assert_eq!(at(&with(&[&disallow]), &uri2), NO_MATCH_DATA);
    // The first group that matches decides.
    assert_eq!(at(&with(&[&disallow, &match_group]), &uri), NO_MATCH_DATA);
    assert_eq!(at(&with(&[&disallow, &match_group]), &uri2), NO_MATCH_DATA);
    let f3 = with(&[&no_match_group, &match_group, &disallow]);
    assert_eq!(at(&f3, &uri), path);
    assert_eq!(at(&f3, &uri2), path);
}

/// CTS `testMatchWithIntentData`.
#[test]
fn match_with_intent_data() {
    let mut f = IntentFilter::default();
    let uri = Uri::parse(URI);
    let m = |f: &IntentFilter, ty, scheme, data: Option<&Uri>, cats: Option<&[String]>| {
        f.matches(Some(ACTION), ty, scheme, data, cats, false, None)
    };
    assert_eq!(m(&f, None, None, None, None), NO_MATCH_ACTION);
    f.add_action(ACTION);
    let empty = MATCH_CATEGORY_EMPTY + MATCH_ADJUSTMENT_NORMAL;
    assert_eq!(m(&f, None, None, None, None), empty);
    assert_eq!(m(&f, None, Some(DATA_SCHEME), None, None), empty);
    assert_eq!(
        f.match_data(None, Some(DATA_SCHEME), Some(&uri), false),
        NO_MATCH_DATA
    );
    assert_eq!(
        m(
            &f,
            Some(DATA_STATIC_TYPE),
            Some(DATA_SCHEME),
            Some(&uri),
            None
        ),
        NO_MATCH_DATA
    );
    f.add_data_scheme(DATA_SCHEME);
    assert_eq!(
        m(
            &f,
            Some(DATA_STATIC_TYPE),
            Some(DATA_SCHEME),
            Some(&uri),
            None
        ),
        NO_MATCH_TYPE
    );
    assert_eq!(
        m(&f, Some(DATA_STATIC_TYPE), Some(""), Some(&uri), None),
        NO_MATCH_DATA
    );
    f.add_data_type(DATA_STATIC_TYPE).unwrap();
    assert_eq!(
        m(
            &f,
            Some(DATA_STATIC_TYPE),
            Some(DATA_SCHEME),
            Some(&uri),
            None
        ),
        MATCH_CATEGORY_TYPE + MATCH_ADJUSTMENT_NORMAL
    );
    assert_eq!(
        m(&f, None, Some(DATA_SCHEME), Some(&uri), None),
        NO_MATCH_TYPE
    );
    assert_eq!(
        m(&f, None, Some(DATA_SCHEME), Some(&uri), Some(&[])),
        NO_MATCH_TYPE
    );
    let cat = [CATEGORY.to_owned()];
    assert_eq!(
        m(
            &f,
            Some(DATA_STATIC_TYPE),
            Some(DATA_SCHEME),
            Some(&uri),
            Some(&cat)
        ),
        NO_MATCH_CATEGORY
    );
    f.add_data_authority(HOST, Some(PORT));
    assert_eq!(
        m(&f, None, Some(DATA_SCHEME), Some(&uri), None),
        NO_MATCH_DATA
    );
    assert_eq!(
        m(
            &f,
            Some(DATA_STATIC_TYPE),
            Some(DATA_SCHEME),
            Some(&uri),
            None
        ),
        NO_MATCH_DATA
    );
    let uri2 = Uri::parse(&format!("{DATA_SCHEME}://{HOST}:{PORT}"));
    f.add_data_path(PatternMatcher::new(DATA_PATH, PATTERN_LITERAL).unwrap());
    assert_eq!(
        m(
            &f,
            Some(DATA_STATIC_TYPE),
            Some(DATA_SCHEME),
            Some(&uri2),
            None
        ),
        NO_MATCH_DATA
    );
    assert_eq!(
        m(
            &f,
            Some(DATA_STATIC_TYPE),
            Some(DATA_SCHEME),
            Some(&uri),
            Some(&[])
        ),
        NO_MATCH_DATA
    );
    assert_eq!(
        m(
            &f,
            Some(DATA_STATIC_TYPE),
            Some(DATA_SCHEME),
            Some(&uri),
            Some(&cat)
        ),
        NO_MATCH_DATA
    );
    f.add_category(CATEGORY);
    assert_eq!(
        m(
            &f,
            Some(DATA_STATIC_TYPE),
            Some(DATA_SCHEME),
            Some(&uri),
            Some(&cat)
        ),
        NO_MATCH_DATA
    );
}

/// CTS `testMatchCategories` and `testMatchDataAuthority`.
#[test]
fn match_categories_and_authority() {
    let mut f = IntentFilter::default();
    assert!(f.match_categories(None));
    assert!(f.match_categories(Some(&[])));
    assert!(!f.match_categories(Some(&["mytest".into()])));
    f.add_category(CATEGORY);
    assert!(f.match_categories(Some(&[CATEGORY.into()])));
    assert!(!f.match_categories(Some(&[CATEGORY.into(), "mytest".into()])));

    let mut f = IntentFilter::default();
    assert_eq!(f.match_data_authority(None, false), NO_MATCH_DATA);
    f.add_data_authority(HOST, Some(PORT));
    let uri = Uri::parse(&format!("http://{HOST}:{PORT}"));
    assert_eq!(
        f.match_data_authority(Some(&uri), false),
        MATCH_CATEGORY_PORT
    );
}

/// CTS `testAddDataType` (the malformed types) and the partial types it
/// keeps by their base.
#[test]
fn malformed_and_partial_types() {
    let mut f = IntentFilter::default();
    for bad in ["", "/", "a/", "/b", "ab"] {
        assert_eq!(f.add_data_type(bad), Err(MalformedMimeType), "{bad}");
    }
    f.add_data_type("image/*").unwrap();
    f.add_data_type("image/*").unwrap();
    f.add_dynamic_data_type("text/plain").unwrap();
    assert_eq!(
        f.types.as_deref(),
        Some(&["image".to_owned(), "text/plain".to_owned()][..])
    );
    assert_eq!(f.static_types.as_deref(), Some(&["image".to_owned()][..]));
    assert!(f.has_static_partial_types && !f.has_dynamic_partial_types);
    f.clear_dynamic_data_types();
    assert_eq!(f.types, f.static_types);
}

/// CTS `PatternMatcherTest.testMatch`.
#[test]
fn pattern_matcher_cts_match() {
    let pm = |p: &str, kind| PatternMatcher::new(p, kind).unwrap();
    assert!(pm("test", PATTERN_LITERAL).matches(Some("test")));
    assert!(!pm("test", PATTERN_LITERAL).matches(Some("test1")));
    assert!(pm("test", PATTERN_PREFIX).matches(Some("testHello")));
    assert!(!pm("test", PATTERN_PREFIX).matches(Some("atestHello")));
    for s in ["testHello", "test", "atestHello"] {
        assert!(!pm("test", -1).matches(Some(s)));
    }
    assert!(pm("", PATTERN_SIMPLE_GLOB).matches(Some("")));
    assert!(pm("....", PATTERN_SIMPLE_GLOB).matches(Some("test")));
    assert!(!pm("d*", PATTERN_SIMPLE_GLOB).matches(Some("test")));
    assert!(!pm("test", PATTERN_LITERAL).matches(None));
}

/// frameworks/base `PatternMatcherTest`: advanced globs.
#[test]
fn advanced_globs() {
    let cases: &[(&str, &[&str], &[&str])] = &[
        (".", &["a", "b"], &[""]),
        ("[a]", &["a"], &["b"]),
        (
            "[.*+{}\\]\\\\[]",
            &[".", "*", "+", "{", "}", "]", "\\", "["],
            &[],
        ),
        ("[a-z]", &["a", "b"], &["A", "1"]),
        ("[a-z][0-9]", &["a1"], &["1a", "aa"]),
        ("[z-a]", &[], &["a", "z", "A"]),
        ("[^0-9]", &["a", "z", "A"], &["9", "5", "0"]),
        ("[\\[a]", &["a", "["], &[]),
        ("\\.", &["."], &["a", "1"]),
        ("a\\+", &["a+"], &["a", "aaaaa"]),
        ("[\\a-\\z]", &["a", "z"], &["A"]),
        ("a", &["a"], &["", "z"]),
        ("az", &["az"], &["", "za"]),
        ("[a-z]*", &["", "a", "abcdefg"], &["abc1", "1abc"]),
        ("[a-z]+", &["a", "abcdefg"], &["", "abc1", "1abc"]),
        ("[a-z]{1}", &["a", "z"], &["", "1", "aa"]),
        ("[a-z]{1,5}", &["a", "zazaz"], &["", "azazaz", "11111"]),
        ("[a-z]{3,}", &["aza", "zazaz", "azazazazazaz"], &["", "aa"]),
        (
            "/[0-9]{4}/[0-9]{2}/[0-9]{2}/[a-zA-Z0-9_]+\\.html",
            &[
                "/2016/09/07/got_this_working.html",
                "/2016/09/07/got_this_working2.html",
            ],
            &[
                "",
                "/2016/09/07/got_this_working2dothtml",
                "/2016/9/7/got_this_working.html",
            ],
        ),
        (
            "/b*a*bar.*",
            &[
                "/babar",
                "/babarfff",
                "/bbaabarfff",
                "/babar?blah",
                "/baaaabar?blah",
            ],
            &["?bar", "/bar", "/baz", "/ba/bar", "/barf", "/", "?blah"],
        ),
    ];
    for &(pattern, yes, no) in cases {
        let pm = PatternMatcher::new(pattern, PATTERN_ADVANCED_GLOB).unwrap();
        for s in yes {
            assert!(pm.matches(Some(s)), "{s:?} should match {pattern:?}");
        }
        for s in no {
            assert!(!pm.matches(Some(s)), "{s:?} should not match {pattern:?}");
        }
    }
    let mut large = String::from("[");
    (0..1024).for_each(|i| large.push_str(&(97 + i % 26).to_string()));
    large.push(']');
    let bad = [
        "[]a]",
        "[a-z",
        "a{,4}",
        "a{0,a}",
        "a{\\1, 2}",
        "[]",
        "a{}",
        "{3,4}",
        "a+{3,4}",
        "*.",
        ".+*",
        "a{3,4",
        "[a",
        "abc\\",
        "+.",
        &large,
    ];
    for pattern in bad {
        assert!(
            PatternMatcher::new(pattern, PATTERN_ADVANCED_GLOB).is_err(),
            "{pattern:?}"
        );
    }
}

/// A filter read from its parcel, written as `IntentFilter.writeToParcel`
/// writes it (CTS `testWriteToParcel`, `testWriteToParcelWithRelRefGroup`).
#[test]
fn reads_a_parcel() {
    let mut p = Parcel::new();
    let strings = |p: &mut Parcel, list: &[&str]| {
        p.write_i32(list.len() as i32);
        list.iter().for_each(|s| p.write_string16(Some(s)));
    };
    strings(&mut p, &[ACTION]);
    p.write_i32(1);
    strings(&mut p, &[CATEGORY]);
    p.write_i32(1);
    strings(&mut p, &[DATA_SCHEME]);
    p.write_i32(1);
    strings(&mut p, &[DATA_STATIC_TYPE]);
    p.write_i32(1);
    strings(&mut p, &[DATA_STATIC_TYPE, DATA_DYNAMIC_TYPE]);
    p.write_i32(1);
    strings(&mut p, &["mime_group"]);
    p.write_i32(0); // scheme-specific parts
    p.write_i32(1); // authorities
    p.write_string16(Some(HOST));
    p.write_string16(Some(HOST));
    p.write_i32(0);
    p.write_i32(80);
    p.write_i32(1); // paths
    p.write_string16(Some(DATA_PATH));
    p.write_i32(PATTERN_PREFIX);
    p.write_i32(-1);
    p.write_i32(5); // priority
    p.write_i32(0);
    p.write_i32(0);
    p.write_i32(1); // auto verify
    p.write_i32(VISIBILITY_EXPLICIT);
    p.write_i32(3); // order
    p.write_i32(0); // extras
    p.write_i32(1); // groups
    p.write_i32(ACTION_ALLOW);
    p.write_i32(2);
    p.write_i32(URI_PART_PATH);
    p.write_i32(PATTERN_LITERAL);
    p.write_string16(Some("/path"));
    p.write_i32(URI_PART_QUERY);
    p.write_i32(PATTERN_SIMPLE_GLOB);
    p.write_string16(Some("q*"));
    let mut r = Reader::new(p.data(), &[]);
    let f = IntentFilter::read(&mut r, &mut Plain).unwrap();
    assert_eq!(r.remaining(), 0);
    assert_eq!(f.actions, [ACTION]);
    assert!(f.has_category(CATEGORY) && f.has_data_scheme(DATA_SCHEME));
    assert_eq!(
        f.authorities.as_ref().unwrap()[0],
        AuthorityEntry::new(HOST, Some("80"))
    );
    assert_eq!(
        f.paths.as_ref().unwrap()[0],
        PatternMatcher::new(DATA_PATH, PATTERN_PREFIX).unwrap()
    );
    assert_eq!(f.types.as_ref().unwrap().len(), 2);
    assert_eq!((f.priority, f.order, f.auto_verify), (5, 3, true));
    assert!(f.is_explicitly_visible_to_instant_app());
    let groups = f.uri_relative_filter_groups.as_ref().unwrap();
    assert_eq!(groups[0].filters.len(), 2);
    // Written back as the original writes it.
    let mut q = Parcel::new();
    f.write(&mut q);
    assert_eq!(q.data(), p.data());
}

/// `readFromXml`, as package-restrictions.xml keeps a preferred
/// activity's filter.
#[test]
fn reads_xml() {
    let xml = br#"<filter autoVerify="true">
        <action name="android.intent.action.VIEW" />
        <cat name="android.intent.category.DEFAULT" />
        <staticType name="image/*" />
        <type name="text/plain" />
        <scheme name="https" />
        <auth host="*.example.com" port="443" />
        <path prefix="/a" />
        <ssp sglob="x.*" />
    </filter>"#;
    let f = IntentFilter::parse(&aim_android_xml::read(xml).unwrap());
    assert!(!f.auto_verify);
    assert!(f.has_action(ACTION_VIEW) && f.has_category("android.intent.category.DEFAULT"));
    assert_eq!(f.static_types.as_deref(), Some(&["image".to_owned()][..]));
    assert_eq!(f.types.as_ref().unwrap().len(), 2);
    let a = &f.authorities.as_ref().unwrap()[0];
    assert_eq!(
        (a.host.as_str(), a.wild, a.port),
        (".example.com", true, 443)
    );
    assert_eq!(f.paths.as_ref().unwrap()[0].kind, PATTERN_PREFIX);
    assert_eq!(f.ssps.as_ref().unwrap()[0].kind, PATTERN_SIMPLE_GLOB);
}

/// `handlesWebUris`, `handleAllWebDataURI`.
#[test]
fn web_uris() {
    let mut f = IntentFilter::default();
    f.add_action(ACTION_VIEW);
    f.add_category(CATEGORY_BROWSABLE);
    assert!(!f.handles_web_uris(false));
    f.add_data_scheme("https");
    assert!(f.handles_web_uris(false) && f.handles_web_uris(true) && f.handle_all_web_data_uri());
    f.add_data_scheme("myapp");
    assert!(f.handles_web_uris(false) && !f.handles_web_uris(true));
    f.add_data_authority("example.com", None);
    assert!(!f.handle_all_web_data_uri());
    f.add_category(CATEGORY_APP_BROWSER);
    assert!(f.handle_all_web_data_uri());
}

/// Hosts compare without case; a wildcard host matches its suffix.
#[test]
fn hosts() {
    let a = AuthorityEntry::new("*.Example.com", None);
    assert_eq!(
        a.matches(&Uri::parse("http://www.example.COM/"), false),
        MATCH_CATEGORY_HOST
    );
    assert_eq!(
        a.matches(&Uri::parse("http://example.com/"), false),
        NO_MATCH_DATA
    );
    assert_eq!(
        a.matches(&Uri::parse("http://x.example.com:1/"), false),
        MATCH_CATEGORY_HOST
    );
    let a = AuthorityEntry::new("host", Some("x"));
    assert_eq!(a.port, -1);
}

/// CTS `testCategories`.
#[test]
fn categories() {
    let filter = Match::new(None, Some(&[Some("category1")][..]), None, None, None, None);
    check(
        &filter,
        &[
            mc(MATCH_CATEGORY_EMPTY, None, None, None, None, false, None),
            mc(
                MATCH_CATEGORY_EMPTY,
                None,
                Some(&[Some("category1")][..]),
                None,
                None,
                false,
                None,
            ),
            mc(
                NO_MATCH_CATEGORY,
                None,
                Some(&[Some("category2")][..]),
                None,
                None,
                false,
                None,
            ),
            mc(
                NO_MATCH_CATEGORY,
                None,
                Some(&[Some("category1"), Some("category2")][..]),
                None,
                None,
                false,
                None,
            ),
        ],
    );
    let filter = Match::new(
        None,
        Some(&[Some("category1"), Some("category2")][..]),
        None,
        None,
        None,
        None,
    );
    check(
        &filter,
        &[
            mc(MATCH_CATEGORY_EMPTY, None, None, None, None, false, None),
            mc(
                MATCH_CATEGORY_EMPTY,
                None,
                Some(&[Some("category1")][..]),
                None,
                None,
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_EMPTY,
                None,
                Some(&[Some("category2")][..]),
                None,
                None,
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_EMPTY,
                None,
                Some(&[Some("category1"), Some("category2")][..]),
                None,
                None,
                false,
                None,
            ),
            mc(
                NO_MATCH_CATEGORY,
                None,
                Some(&[Some("category3")][..]),
                None,
                None,
                false,
                None,
            ),
            mc(
                NO_MATCH_CATEGORY,
                None,
                Some(&[Some("category1"), Some("category2"), Some("category3")][..]),
                None,
                None,
                false,
                None,
            ),
        ],
    );
}

/// CTS `testMimeTypes`.
#[test]
fn mime_types() {
    let filter = Match::new(
        None,
        None,
        Some(&[Some("which1/what1")][..]),
        None,
        None,
        None,
    );
    check(
        &filter,
        &[
            mc(NO_MATCH_TYPE, None, None, None, None, false, None),
            mc(
                MATCH_CATEGORY_TYPE,
                None,
                None,
                Some("which1/what1"),
                None,
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_TYPE,
                None,
                None,
                Some("which1/*"),
                None,
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_TYPE,
                None,
                None,
                Some("*/*"),
                None,
                false,
                None,
            ),
            mc(
                NO_MATCH_TYPE,
                None,
                None,
                Some("which2/what2"),
                None,
                false,
                None,
            ),
            mc(
                NO_MATCH_TYPE,
                None,
                None,
                Some("which2/*"),
                None,
                false,
                None,
            ),
            mc(
                NO_MATCH_TYPE,
                None,
                None,
                Some("which1/what2"),
                None,
                false,
                None,
            ),
        ],
    );
    let filter = Match::new(
        None,
        None,
        Some(&[Some("which1/what1"), Some("which2/what2")][..]),
        None,
        None,
        None,
    );
    check(
        &filter,
        &[
            mc(NO_MATCH_TYPE, None, None, None, None, false, None),
            mc(
                MATCH_CATEGORY_TYPE,
                None,
                None,
                Some("which1/what1"),
                None,
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_TYPE,
                None,
                None,
                Some("which1/*"),
                None,
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_TYPE,
                None,
                None,
                Some("*/*"),
                None,
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_TYPE,
                None,
                None,
                Some("which2/what2"),
                None,
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_TYPE,
                None,
                None,
                Some("which2/*"),
                None,
                false,
                None,
            ),
            mc(
                NO_MATCH_TYPE,
                None,
                None,
                Some("which1/what2"),
                None,
                false,
                None,
            ),
            mc(
                NO_MATCH_TYPE,
                None,
                None,
                Some("which3/what3"),
                None,
                false,
                None,
            ),
        ],
    );
    let filter = Match::new(None, None, Some(&[Some("which1/*")][..]), None, None, None);
    check(
        &filter,
        &[
            mc(NO_MATCH_TYPE, None, None, None, None, false, None),
            mc(
                MATCH_CATEGORY_TYPE,
                None,
                None,
                Some("which1/what1"),
                None,
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_TYPE,
                None,
                None,
                Some("which1/*"),
                None,
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_TYPE,
                None,
                None,
                Some("*/*"),
                None,
                false,
                None,
            ),
            mc(
                NO_MATCH_TYPE,
                None,
                None,
                Some("which2/what2"),
                None,
                false,
                None,
            ),
            mc(
                NO_MATCH_TYPE,
                None,
                None,
                Some("which2/*"),
                None,
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_TYPE,
                None,
                None,
                Some("which1/what2"),
                None,
                false,
                None,
            ),
            mc(
                NO_MATCH_TYPE,
                None,
                None,
                Some("which3/what3"),
                None,
                false,
                None,
            ),
        ],
    );
    let filter = Match::new(None, None, Some(&[Some("*/*")][..]), None, None, None);
    check(
        &filter,
        &[
            mc(NO_MATCH_TYPE, None, None, None, None, false, None),
            mc(
                MATCH_CATEGORY_TYPE,
                None,
                None,
                Some("which1/what1"),
                None,
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_TYPE,
                None,
                None,
                Some("which1/*"),
                None,
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_TYPE,
                None,
                None,
                Some("*/*"),
                None,
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_TYPE,
                None,
                None,
                Some("which2/what2"),
                None,
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_TYPE,
                None,
                None,
                Some("which2/*"),
                None,
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_TYPE,
                None,
                None,
                Some("which1/what2"),
                None,
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_TYPE,
                None,
                None,
                Some("which3/what3"),
                None,
                false,
                None,
            ),
        ],
    );
}

/// CTS `testDynamicMimeTypes`.
#[test]
fn dynamic_mime_types() {
    let filter = Match::new(None, None, None, None, None, None)
        .dynamic_types(Some(&[Some("which1/what1")][..]));
    check(
        &filter,
        &[
            mc(NO_MATCH_TYPE, None, None, None, None, false, None),
            mc(
                MATCH_CATEGORY_TYPE,
                None,
                None,
                Some("which1/what1"),
                None,
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_TYPE,
                None,
                None,
                Some("which1/*"),
                None,
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_TYPE,
                None,
                None,
                Some("*/*"),
                None,
                false,
                None,
            ),
            mc(
                NO_MATCH_TYPE,
                None,
                None,
                Some("which2/what2"),
                None,
                false,
                None,
            ),
            mc(
                NO_MATCH_TYPE,
                None,
                None,
                Some("which2/*"),
                None,
                false,
                None,
            ),
            mc(
                NO_MATCH_TYPE,
                None,
                None,
                Some("which1/what2"),
                None,
                false,
                None,
            ),
        ],
    );
    let filter = Match::new(None, None, None, None, None, None)
        .dynamic_types(Some(&[Some("which1/what1"), Some("which2/what2")][..]));
    check(
        &filter,
        &[
            mc(NO_MATCH_TYPE, None, None, None, None, false, None),
            mc(
                MATCH_CATEGORY_TYPE,
                None,
                None,
                Some("which1/what1"),
                None,
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_TYPE,
                None,
                None,
                Some("which1/*"),
                None,
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_TYPE,
                None,
                None,
                Some("*/*"),
                None,
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_TYPE,
                None,
                None,
                Some("which2/what2"),
                None,
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_TYPE,
                None,
                None,
                Some("which2/*"),
                None,
                false,
                None,
            ),
            mc(
                NO_MATCH_TYPE,
                None,
                None,
                Some("which1/what2"),
                None,
                false,
                None,
            ),
            mc(
                NO_MATCH_TYPE,
                None,
                None,
                Some("which3/what3"),
                None,
                false,
                None,
            ),
        ],
    );
    let filter =
        Match::new(None, None, None, None, None, None).dynamic_types(Some(&[Some("which1/*")][..]));
    check(
        &filter,
        &[
            mc(NO_MATCH_TYPE, None, None, None, None, false, None),
            mc(
                MATCH_CATEGORY_TYPE,
                None,
                None,
                Some("which1/what1"),
                None,
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_TYPE,
                None,
                None,
                Some("which1/*"),
                None,
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_TYPE,
                None,
                None,
                Some("*/*"),
                None,
                false,
                None,
            ),
            mc(
                NO_MATCH_TYPE,
                None,
                None,
                Some("which2/what2"),
                None,
                false,
                None,
            ),
            mc(
                NO_MATCH_TYPE,
                None,
                None,
                Some("which2/*"),
                None,
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_TYPE,
                None,
                None,
                Some("which1/what2"),
                None,
                false,
                None,
            ),
            mc(
                NO_MATCH_TYPE,
                None,
                None,
                Some("which3/what3"),
                None,
                false,
                None,
            ),
        ],
    );
    let filter =
        Match::new(None, None, None, None, None, None).dynamic_types(Some(&[Some("*/*")][..]));
    check(
        &filter,
        &[
            mc(NO_MATCH_TYPE, None, None, None, None, false, None),
            mc(
                MATCH_CATEGORY_TYPE,
                None,
                None,
                Some("which1/what1"),
                None,
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_TYPE,
                None,
                None,
                Some("which1/*"),
                None,
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_TYPE,
                None,
                None,
                Some("*/*"),
                None,
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_TYPE,
                None,
                None,
                Some("which2/what2"),
                None,
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_TYPE,
                None,
                None,
                Some("which2/*"),
                None,
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_TYPE,
                None,
                None,
                Some("which1/what2"),
                None,
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_TYPE,
                None,
                None,
                Some("which3/what3"),
                None,
                false,
                None,
            ),
        ],
    );
}

/// CTS `testClearDynamicMimeTypesWithStaticType`.
#[test]
fn clear_dynamic_mime_types_with_static_type() {
    let mut filter = Match::new(None, None, None, None, None, None)
        .types(Some(&[Some("which1/what1")][..]))
        .dynamic_types(Some(&[Some("which2/what2")][..]));
    check(
        &filter,
        &[
            mc(NO_MATCH_TYPE, None, None, None, None, false, None),
            mc(
                MATCH_CATEGORY_TYPE,
                None,
                None,
                Some("which1/what1"),
                None,
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_TYPE,
                None,
                None,
                Some("which1/*"),
                None,
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_TYPE,
                None,
                None,
                Some("*/*"),
                None,
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_TYPE,
                None,
                None,
                Some("which2/what2"),
                None,
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_TYPE,
                None,
                None,
                Some("which2/*"),
                None,
                false,
                None,
            ),
            mc(
                NO_MATCH_TYPE,
                None,
                None,
                Some("which1/what2"),
                None,
                false,
                None,
            ),
            mc(
                NO_MATCH_TYPE,
                None,
                None,
                Some("which3/what3"),
                None,
                false,
                None,
            ),
        ],
    );
    filter.0.clear_dynamic_data_types();
    check(
        &filter,
        &[
            mc(NO_MATCH_TYPE, None, None, None, None, false, None),
            mc(
                MATCH_CATEGORY_TYPE,
                None,
                None,
                Some("which1/what1"),
                None,
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_TYPE,
                None,
                None,
                Some("which1/*"),
                None,
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_TYPE,
                None,
                None,
                Some("*/*"),
                None,
                false,
                None,
            ),
            mc(
                NO_MATCH_TYPE,
                None,
                None,
                Some("which2/what2"),
                None,
                false,
                None,
            ),
            mc(
                NO_MATCH_TYPE,
                None,
                None,
                Some("which2/*"),
                None,
                false,
                None,
            ),
            mc(
                NO_MATCH_TYPE,
                None,
                None,
                Some("which1/what2"),
                None,
                false,
                None,
            ),
        ],
    );
}

/// CTS `testClearDynamicMimeTypesWithAction`.
#[test]
fn clear_dynamic_mime_types_with_action() {
    let mut filter = Match::new(None, None, None, None, None, None)
        .actions(Some(&[Some("action1")][..]))
        .dynamic_types(Some(&[Some("which1/what1")][..]));
    check(
        &filter,
        &[
            mc(NO_MATCH_TYPE, None, None, None, None, false, None),
            mc(
                MATCH_CATEGORY_TYPE,
                None,
                None,
                Some("which1/what1"),
                None,
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_TYPE,
                None,
                None,
                Some("which1/*"),
                None,
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_TYPE,
                None,
                None,
                Some("*/*"),
                None,
                false,
                None,
            ),
            mc(
                NO_MATCH_TYPE,
                None,
                None,
                Some("which2/what2"),
                None,
                false,
                None,
            ),
            mc(
                NO_MATCH_TYPE,
                None,
                None,
                Some("which2/*"),
                None,
                false,
                None,
            ),
            mc(
                NO_MATCH_TYPE,
                None,
                None,
                Some("which1/what2"),
                None,
                false,
                None,
            ),
        ],
    );
    filter.0.clear_dynamic_data_types();
    check(
        &filter,
        &[
            mc(MATCH_CATEGORY_EMPTY, None, None, None, None, false, None),
            mc(
                MATCH_CATEGORY_EMPTY,
                Some("action1"),
                None,
                None,
                None,
                false,
                None,
            ),
            mc(
                NO_MATCH_TYPE,
                Some("action1"),
                None,
                Some("which2/what2"),
                None,
                false,
                None,
            ),
        ],
    );
}

/// CTS `testDataSchemes`.
#[test]
fn data_schemes() {
    let filter = Match::new(None, None, None, Some(&[Some("scheme1")][..]), None, None);
    check(
        &filter,
        &[
            mc(NO_MATCH_DATA, None, None, None, None, false, None),
            mc(
                MATCH_CATEGORY_SCHEME,
                None,
                None,
                None,
                Some("scheme1:foo"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme2:foo"),
                false,
                None,
            ),
        ],
    );
    let filter = Match::new(
        None,
        None,
        None,
        Some(&[Some("scheme1"), Some("scheme2")][..]),
        None,
        None,
    );
    check(
        &filter,
        &[
            mc(NO_MATCH_DATA, None, None, None, None, false, None),
            mc(
                MATCH_CATEGORY_SCHEME,
                None,
                None,
                None,
                Some("scheme1:foo"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_SCHEME,
                None,
                None,
                None,
                Some("scheme2:foo"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme3:foo"),
                false,
                None,
            ),
        ],
    );
}

/// CTS `testSchemeSpecificParts`.
#[test]
fn scheme_specific_parts() {
    let filter = Match::new(None, None, None, Some(&[Some("scheme")][..]), None, None)
        .paths(None, None)
        .ssps(
            Some(&[Some("ssp1"), Some("2ssp")][..]),
            Some(&[PATTERN_LITERAL, PATTERN_LITERAL][..]),
        );
    check(
        &filter,
        &[
            mc(NO_MATCH_DATA, None, None, None, None, false, None),
            mc(
                MATCH_CATEGORY_SCHEME_SPECIFIC_PART,
                None,
                None,
                None,
                Some("scheme:ssp1"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_SCHEME_SPECIFIC_PART,
                None,
                None,
                None,
                Some("scheme:2ssp"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme:ssp"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme:ssp12"),
                false,
                None,
            ),
        ],
    );
    let filter = Match::new(None, None, None, Some(&[Some("scheme")][..]), None, None)
        .paths(None, None)
        .ssps(
            Some(&[Some("ssp1"), Some("2ssp")][..]),
            Some(&[PATTERN_PREFIX, PATTERN_PREFIX][..]),
        );
    check(
        &filter,
        &[
            mc(NO_MATCH_DATA, None, None, None, None, false, None),
            mc(
                MATCH_CATEGORY_SCHEME_SPECIFIC_PART,
                None,
                None,
                None,
                Some("scheme:ssp1"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_SCHEME_SPECIFIC_PART,
                None,
                None,
                None,
                Some("scheme:2ssp"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme:ssp"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_SCHEME_SPECIFIC_PART,
                None,
                None,
                None,
                Some("scheme:ssp12"),
                false,
                None,
            ),
        ],
    );
    let filter = Match::new(None, None, None, Some(&[Some("scheme")][..]), None, None)
        .paths(None, None)
        .ssps(
            Some(&[Some("p1"), Some("sp"), Some(".file")][..]),
            Some(&[PATTERN_SUFFIX, PATTERN_SUFFIX, PATTERN_SUFFIX][..]),
        );
    check(
        &filter,
        &[
            mc(NO_MATCH_DATA, None, None, None, None, false, None),
            mc(
                MATCH_CATEGORY_SCHEME_SPECIFIC_PART,
                None,
                None,
                None,
                Some("scheme:ssp1"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_SCHEME_SPECIFIC_PART,
                None,
                None,
                None,
                Some("scheme:2ssp"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_SCHEME_SPECIFIC_PART,
                None,
                None,
                None,
                Some("scheme:something.file"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_SCHEME_SPECIFIC_PART,
                None,
                None,
                None,
                Some("scheme:ssp"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme:ssp12"),
                false,
                None,
            ),
        ],
    );
    check_all(
        &[
            scheme_and_ssp("scheme", "ssp.*", PATTERN_ADVANCED_GLOB),
            scheme_and_ssp("scheme", "ssp.*", PATTERN_SIMPLE_GLOB),
        ],
        &[
            mc(NO_MATCH_DATA, None, None, None, None, false, None),
            mc(
                MATCH_CATEGORY_SCHEME_SPECIFIC_PART,
                None,
                None,
                None,
                Some("scheme:ssp1"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_SCHEME_SPECIFIC_PART,
                None,
                None,
                None,
                Some("scheme:ssp"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme:ss"),
                false,
                None,
            ),
        ],
    );
    check_all(
        &[
            scheme_and_ssp("scheme", ".*", PATTERN_ADVANCED_GLOB),
            scheme_and_ssp("scheme", ".*", PATTERN_SIMPLE_GLOB),
        ],
        &[
            mc(NO_MATCH_DATA, None, None, None, None, false, None),
            mc(
                MATCH_CATEGORY_SCHEME_SPECIFIC_PART,
                None,
                None,
                None,
                Some("scheme:ssp1"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_SCHEME_SPECIFIC_PART,
                None,
                None,
                None,
                Some("scheme:ssp"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_SCHEME_SPECIFIC_PART,
                None,
                None,
                None,
                Some("scheme:"),
                false,
                None,
            ),
        ],
    );
    check_all(
        &[
            scheme_and_ssp("scheme", "a1*b", PATTERN_ADVANCED_GLOB),
            scheme_and_ssp("scheme", "a1*b", PATTERN_SIMPLE_GLOB),
        ],
        &[
            mc(NO_MATCH_DATA, None, None, None, None, false, None),
            mc(
                MATCH_CATEGORY_SCHEME_SPECIFIC_PART,
                None,
                None,
                None,
                Some("scheme:ab"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_SCHEME_SPECIFIC_PART,
                None,
                None,
                None,
                Some("scheme:a1b"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_SCHEME_SPECIFIC_PART,
                None,
                None,
                None,
                Some("scheme:a11b"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme:a2b"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme:a1bc"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme:a"),
                false,
                None,
            ),
        ],
    );
    check_all(
        &[
            scheme_and_ssp("scheme", "a1*", PATTERN_ADVANCED_GLOB),
            scheme_and_ssp("scheme", "a1*", PATTERN_SIMPLE_GLOB),
        ],
        &[
            mc(NO_MATCH_DATA, None, None, None, None, false, None),
            mc(
                MATCH_CATEGORY_SCHEME_SPECIFIC_PART,
                None,
                None,
                None,
                Some("scheme:a1"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme:ab"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_SCHEME_SPECIFIC_PART,
                None,
                None,
                None,
                Some("scheme:a11"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme:a1b"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_SCHEME_SPECIFIC_PART,
                None,
                None,
                None,
                Some("scheme:a11"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme:a2"),
                false,
                None,
            ),
        ],
    );
    check_all(
        &[
            scheme_and_ssp("scheme", "a\\.*b", PATTERN_ADVANCED_GLOB),
            scheme_and_ssp("scheme", "a\\.*b", PATTERN_SIMPLE_GLOB),
        ],
        &[
            mc(NO_MATCH_DATA, None, None, None, None, false, None),
            mc(
                MATCH_CATEGORY_SCHEME_SPECIFIC_PART,
                None,
                None,
                None,
                Some("scheme:ab"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_SCHEME_SPECIFIC_PART,
                None,
                None,
                None,
                Some("scheme:a.b"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_SCHEME_SPECIFIC_PART,
                None,
                None,
                None,
                Some("scheme:a..b"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme:a2b"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme:a.bc"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme:"),
                false,
                None,
            ),
        ],
    );
    check_all(
        &[
            scheme_and_ssp("scheme", "a[.1-2]*b", PATTERN_ADVANCED_GLOB),
            scheme_and_ssp("scheme", "a.*b", PATTERN_SIMPLE_GLOB),
        ],
        &[
            mc(NO_MATCH_DATA, None, None, None, None, false, None),
            mc(
                MATCH_CATEGORY_SCHEME_SPECIFIC_PART,
                None,
                None,
                None,
                Some("scheme:ab"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_SCHEME_SPECIFIC_PART,
                None,
                None,
                None,
                Some("scheme:a.b"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_SCHEME_SPECIFIC_PART,
                None,
                None,
                None,
                Some("scheme:a.1b"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_SCHEME_SPECIFIC_PART,
                None,
                None,
                None,
                Some("scheme:a2b"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme:a.bc"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme:"),
                false,
                None,
            ),
        ],
    );
    check_all(
        &[
            scheme_and_ssp("scheme", "a.*", PATTERN_ADVANCED_GLOB),
            scheme_and_ssp("scheme", "a.*", PATTERN_SIMPLE_GLOB),
        ],
        &[
            mc(NO_MATCH_DATA, None, None, None, None, false, None),
            mc(
                MATCH_CATEGORY_SCHEME_SPECIFIC_PART,
                None,
                None,
                None,
                Some("scheme:ab"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_SCHEME_SPECIFIC_PART,
                None,
                None,
                None,
                Some("scheme:a.b"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_SCHEME_SPECIFIC_PART,
                None,
                None,
                None,
                Some("scheme:a.1b"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_SCHEME_SPECIFIC_PART,
                None,
                None,
                None,
                Some("scheme:a2b"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_SCHEME_SPECIFIC_PART,
                None,
                None,
                None,
                Some("scheme:a.bc"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme:"),
                false,
                None,
            ),
        ],
    );
    check_all(
        &[
            scheme_and_ssp("scheme", "a.\\*b", PATTERN_ADVANCED_GLOB),
            scheme_and_ssp("scheme", "a.\\*b", PATTERN_SIMPLE_GLOB),
        ],
        &[
            mc(NO_MATCH_DATA, None, None, None, None, false, None),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme:ab"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_SCHEME_SPECIFIC_PART,
                None,
                None,
                None,
                Some("scheme:a.*b"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_SCHEME_SPECIFIC_PART,
                None,
                None,
                None,
                Some("scheme:a1*b"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme:a2b"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme:a.bc"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme:"),
                false,
                None,
            ),
        ],
    );
    check_all(
        &[
            scheme_and_ssp("scheme", "a.\\*", PATTERN_ADVANCED_GLOB),
            scheme_and_ssp("scheme", "a.\\*", PATTERN_SIMPLE_GLOB),
        ],
        &[
            mc(NO_MATCH_DATA, None, None, None, None, false, None),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme:ab"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_SCHEME_SPECIFIC_PART,
                None,
                None,
                None,
                Some("scheme:a.*"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_SCHEME_SPECIFIC_PART,
                None,
                None,
                None,
                Some("scheme:a1*"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme:a1b"),
                false,
                None,
            ),
        ],
    );
}

/// CTS `testSchemeSpecificPartsWithWildCards`.
#[test]
fn scheme_specific_parts_with_wild_cards() {
    let filter = Match::new(None, None, None, Some(&[Some("scheme")][..]), None, None)
        .paths(None, None)
        .ssps(
            Some(&[Some("ssp1")][..]),
            Some(&[PATTERN_LITERAL, PATTERN_LITERAL][..]),
        );
    check(
        &filter,
        &[
            mc(NO_MATCH_DATA, None, None, None, None, true, None),
            mc(
                MATCH_CATEGORY_SCHEME_SPECIFIC_PART,
                None,
                None,
                None,
                Some("scheme:ssp1"),
                true,
                None,
            ),
            mc(
                MATCH_CATEGORY_SCHEME_SPECIFIC_PART,
                None,
                None,
                None,
                Some("*:ssp1"),
                true,
                None,
            ),
            mc(
                MATCH_CATEGORY_SCHEME_SPECIFIC_PART,
                None,
                None,
                None,
                Some("scheme:*"),
                true,
                None,
            ),
            mc(
                MATCH_CATEGORY_SCHEME_SPECIFIC_PART,
                None,
                None,
                None,
                Some("*:*"),
                true,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme:ssp12"),
                true,
                None,
            ),
        ],
    );
    check(
        &filter,
        &[
            mc(NO_MATCH_DATA, None, None, None, Some("*:ssp1"), false, None),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme:*"),
                false,
                None,
            ),
            mc(NO_MATCH_DATA, None, None, None, Some("*:*"), false, None),
        ],
    );
}

/// CTS `testAuthorities`.
#[test]
fn authorities() {
    let filter = Match::new(
        None,
        None,
        None,
        Some(&[Some("scheme1")][..]),
        Some(&[Some("authority1")][..]),
        Some(&[None][..]),
    );
    check(
        &filter,
        &[
            mc(NO_MATCH_DATA, None, None, None, None, false, None),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme1:foo"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_HOST,
                None,
                None,
                None,
                Some("scheme1://authority1/"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme1://authority2/"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_HOST,
                None,
                None,
                None,
                Some("scheme1://authority1:100/"),
                false,
                None,
            ),
        ],
    );
    let filter = Match::new(
        None,
        None,
        None,
        Some(&[Some("scheme1")][..]),
        Some(&[Some("authority1")][..]),
        Some(&[Some("100")][..]),
    );
    check(
        &filter,
        &[
            mc(NO_MATCH_DATA, None, None, None, None, false, None),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme1:foo"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme1://authority1/"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme1://authority2/"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_PORT,
                None,
                None,
                None,
                Some("scheme1://authority1:100/"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme1://authority1:200/"),
                false,
                None,
            ),
        ],
    );
    let filter = Match::new(
        None,
        None,
        None,
        Some(&[Some("scheme1")][..]),
        Some(&[Some("authority1"), Some("authority2")][..]),
        Some(&[Some("100"), None][..]),
    );
    check(
        &filter,
        &[
            mc(NO_MATCH_DATA, None, None, None, None, false, None),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme1:foo"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme1://authority1/"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_HOST,
                None,
                None,
                None,
                Some("scheme1://authority2/"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_PORT,
                None,
                None,
                None,
                Some("scheme1://authority1:100/"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme1://authority1:200/"),
                false,
                None,
            ),
        ],
    );
}

/// CTS `testAuthoritiesWithWildcards`.
#[test]
fn authorities_with_wildcards() {
    let filter = Match::new(
        None,
        None,
        None,
        Some(&[Some("scheme1")][..]),
        Some(&[Some("authority1")][..]),
        Some(&[None][..]),
    );
    check(
        &filter,
        &[
            mc(NO_MATCH_DATA, None, None, None, None, true, None),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme1:*"),
                true,
                None,
            ),
            mc(
                MATCH_CATEGORY_HOST,
                None,
                None,
                None,
                Some("scheme1://*/"),
                true,
                None,
            ),
            mc(
                MATCH_CATEGORY_HOST,
                None,
                None,
                None,
                Some("scheme1://*:100/"),
                true,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme1://*/"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme1://*:100/"),
                false,
                None,
            ),
        ],
    );
    let filter = Match::new(
        None,
        None,
        None,
        Some(&[Some("scheme1")][..]),
        Some(&[Some("authority1")][..]),
        Some(&[Some("100")][..]),
    );
    check(
        &filter,
        &[
            mc(NO_MATCH_DATA, None, None, None, None, true, None),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme1:*"),
                true,
                None,
            ),
            mc(
                MATCH_CATEGORY_HOST,
                None,
                None,
                None,
                Some("scheme1://*/"),
                true,
                None,
            ),
            mc(
                MATCH_CATEGORY_HOST,
                None,
                None,
                None,
                Some("scheme1://*:100/"),
                true,
                None,
            ),
            mc(
                MATCH_CATEGORY_HOST,
                None,
                None,
                None,
                Some("scheme1://*:200/"),
                true,
                None,
            ),
            mc(NO_MATCH_DATA, None, None, None, Some("*:foo"), true, None),
            mc(
                MATCH_CATEGORY_HOST,
                None,
                None,
                None,
                Some("*://authority1/"),
                true,
                None,
            ),
            mc(
                MATCH_CATEGORY_HOST,
                None,
                None,
                None,
                Some("*://authority1:100/"),
                true,
                None,
            ),
            mc(
                MATCH_CATEGORY_HOST,
                None,
                None,
                None,
                Some("*://authority1:200/"),
                true,
                None,
            ),
            mc(NO_MATCH_DATA, None, None, None, Some("*:*"), true, None),
            mc(
                MATCH_CATEGORY_HOST,
                None,
                None,
                None,
                Some("*://*/"),
                true,
                None,
            ),
            mc(
                MATCH_CATEGORY_HOST,
                None,
                None,
                None,
                Some("*://*:100/"),
                true,
                None,
            ),
            mc(
                MATCH_CATEGORY_HOST,
                None,
                None,
                None,
                Some("*://*:200/"),
                true,
                None,
            ),
        ],
    );
    check(
        &filter,
        &[
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme1://*/"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme1://*:100/"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("*://authority1:100/"),
                false,
                None,
            ),
            mc(NO_MATCH_DATA, None, None, None, Some("*://*/"), false, None),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("*://*:100/"),
                false,
                None,
            ),
        ],
    );
    let filter = Match::new(
        None,
        None,
        None,
        Some(&[Some("scheme1")][..]),
        Some(&[Some("*")][..]),
        None,
    );
    check(
        &filter,
        &[
            mc(
                MATCH_CATEGORY_HOST,
                None,
                None,
                None,
                Some("scheme1://"),
                true,
                None,
            ),
            mc(
                MATCH_CATEGORY_HOST,
                None,
                None,
                None,
                Some("scheme1://"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_HOST,
                None,
                None,
                None,
                Some("scheme1://*"),
                true,
                None,
            ),
        ],
    );
}

/// CTS `testAppEnumerationMatchesMimeGroups`.
#[test]
fn app_enumeration_matches_mime_groups() {
    let filter = Match::new(
        Some(&[Some(ACTION)][..]),
        None,
        None,
        Some(&[Some("scheme1")][..]),
        Some(&[Some("authority1")][..]),
        None,
    )
    .groups(Some(&[Some("test")][..]));
    check(
        &filter,
        &[
            mc(
                MATCH_CATEGORY_TYPE,
                Some(ACTION),
                None,
                Some("img/jpeg"),
                Some("scheme1://authority1"),
                true,
                None,
            ),
            mc(
                MATCH_CATEGORY_TYPE,
                Some(ACTION),
                None,
                None,
                Some("scheme1://authority1"),
                true,
                None,
            ),
            mc(
                MATCH_CATEGORY_TYPE,
                Some(ACTION),
                None,
                Some("*/*"),
                Some("scheme1://authority1"),
                true,
                None,
            ),
        ],
    );
}

/// CTS `testActions`.
#[test]
fn actions() {
    let filter = Match::new(Some(&[Some("action1")][..]), None, None, None, None, None);
    check(
        &filter,
        &[
            mc(MATCH_CATEGORY_EMPTY, None, None, None, None, false, None),
            mc(
                MATCH_CATEGORY_EMPTY,
                Some("action1"),
                None,
                None,
                None,
                false,
                None,
            ),
            mc(
                NO_MATCH_ACTION,
                Some("action2"),
                None,
                None,
                None,
                false,
                None,
            ),
        ],
    );
    let filter = Match::new(
        Some(&[Some("action1"), Some("action2")][..]),
        None,
        None,
        None,
        None,
        None,
    );
    check(
        &filter,
        &[
            mc(MATCH_CATEGORY_EMPTY, None, None, None, None, false, None),
            mc(
                MATCH_CATEGORY_EMPTY,
                Some("action1"),
                None,
                None,
                None,
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_EMPTY,
                Some("action2"),
                None,
                None,
                None,
                false,
                None,
            ),
            mc(
                NO_MATCH_ACTION,
                Some("action3"),
                None,
                None,
                None,
                false,
                None,
            ),
            mc(
                NO_MATCH_ACTION,
                Some("action1"),
                None,
                None,
                None,
                false,
                Some(&["action1", "action2"][..]),
            ),
            mc(
                NO_MATCH_ACTION,
                Some("action2"),
                None,
                None,
                None,
                false,
                Some(&["action1", "action2"][..]),
            ),
            mc(
                MATCH_CATEGORY_EMPTY,
                Some("action1"),
                None,
                None,
                None,
                false,
                Some(&["action2"][..]),
            ),
        ],
    );
}

/// CTS `testActionWildCards`.
#[test]
fn action_wild_cards() {
    let filter = Match::new(
        Some(&[Some("action1"), Some("action2")][..]),
        None,
        None,
        None,
        None,
        None,
    );
    check(
        &filter,
        &[
            mc(MATCH_CATEGORY_EMPTY, None, None, None, None, true, None),
            mc(
                MATCH_CATEGORY_EMPTY,
                Some("*"),
                None,
                None,
                None,
                true,
                None,
            ),
            mc(
                MATCH_CATEGORY_EMPTY,
                Some("*"),
                None,
                None,
                None,
                true,
                Some(&["action1"][..]),
            ),
            mc(
                NO_MATCH_ACTION,
                Some("*"),
                None,
                None,
                None,
                true,
                Some(&["action1", "action2"][..]),
            ),
            mc(
                NO_MATCH_ACTION,
                Some("action3"),
                None,
                None,
                None,
                true,
                None,
            ),
        ],
    );
    check(
        &filter,
        &[mc(
            NO_MATCH_ACTION,
            Some("*"),
            None,
            None,
            None,
            false,
            None,
        )],
    );
}

/// CTS `testAppEnumerationContactProviders`.
#[test]
fn app_enumeration_contact_providers() {
    let filter = Match::new(
        Some(&[Some("android.intent.action.VIEW")][..]),
        Some(&[Some("android.intent.category.DEFAULT")][..]),
        Some(&[Some("vnd.android.cursor.item/vnd.com.someapp.profile")][..]),
        Some(&[Some("content")][..]),
        Some(&[Some("com.android.contacts")][..]),
        None,
    );
    check(
        &filter,
        &[mc(
            MATCH_CATEGORY_TYPE,
            Some("android.intent.action.VIEW"),
            None,
            Some("vnd.android.cursor.item/*"),
            Some("content://com.android.contacts"),
            true,
            None,
        )],
    );
}

/// CTS `testAppEnumerationDocumentEditor`.
#[test]
fn app_enumeration_document_editor() {
    let filter = Match::new(
        Some(
            &[
                Some("android.intent.action.VIEW"),
                Some("android.intent.action.EDIT"),
                Some("com.app.android.intent.action.APP_EDIT"),
                Some("com.app.android.intent.action.APP_VIEW"),
            ][..],
        ),
        Some(&[Some("android.intent.category.DEFAULT")][..]),
        Some(
            &[
                Some("application/msword"),
                Some("application/vnd.oasis.opendocument.text"),
                Some("application/rtf"),
                Some("text/rtf"),
                Some("text/plain"),
                Some("application/pdf"),
                Some("application/x-pdf"),
                Some("application/docm"),
            ][..],
        ),
        None,
        None,
        None,
    );
    check(
        &filter,
        &[mc(
            MATCH_CATEGORY_TYPE,
            Some("android.intent.action.VIEW"),
            Some(&[Some("android.intent.category.DEFAULT")][..]),
            Some("*/*"),
            Some("content://com.example.fileprovider"),
            true,
            None,
        )],
    );
}

/// CTS `testAppEnumerationDeepLinks`.
#[test]
fn app_enumeration_deep_links() {
    let filter = Match::new(
        Some(&[Some("android.intent.action.VIEW")][..]),
        Some(
            &[
                Some("android.intent.category.DEFAULT"),
                Some("android.intent.category.BROWSABLE"),
            ][..],
        ),
        None,
        Some(&[Some("http"), Some("https")][..]),
        Some(&[Some("arbitrary-site.com")][..]),
        None,
    );
    check(
        &filter,
        &[mc(
            MATCH_CATEGORY_HOST,
            Some("android.intent.action.VIEW"),
            Some(&[Some("android.intent.category.BROWSABLE")][..]),
            None,
            Some("https://*"),
            true,
            None,
        )],
    );
    check(
        &filter,
        &[mc(
            MATCH_CATEGORY_HOST,
            Some("android.intent.action.VIEW"),
            Some(&[Some("android.intent.category.BROWSABLE")][..]),
            None,
            Some("http://*"),
            true,
            None,
        )],
    );
}

/// CTS `testAppEnumerationCustomShareSheet`.
#[test]
fn app_enumeration_custom_share_sheet() {
    let filter = Match::new(
        Some(&[Some("android.intent.action.SEND")][..]),
        Some(&[Some("android.intent.category.DEFAULT")][..]),
        Some(&[Some("*/*")][..]),
        None,
        None,
        None,
    );
    check(
        &filter,
        &[mc(
            MATCH_CATEGORY_TYPE,
            Some("android.intent.action.SEND"),
            None,
            Some("image/jpeg"),
            Some("content://com.example.fileprovider"),
            true,
            None,
        )],
    );
    check(
        &filter,
        &[mc(
            MATCH_CATEGORY_TYPE,
            Some("android.intent.action.SEND"),
            None,
            Some("image/jpeg"),
            Some("content:"),
            true,
            None,
        )],
    );
}

/// CTS `testAppEnumerationNoHostMatchesWildcardHost`.
#[test]
fn app_enumeration_no_host_matches_wildcard_host() {
    let filter = Match::new(
        Some(&[Some("android.intent.action.VIEW")][..]),
        Some(&[Some("android.intent.category.BROWSABLE")][..]),
        None,
        Some(&[Some("http"), Some("https")][..]),
        Some(&[Some("*")][..]),
        None,
    );
    check(
        &filter,
        &[mc(
            MATCH_CATEGORY_HOST,
            Some("android.intent.action.VIEW"),
            Some(&[Some("android.intent.category.BROWSABLE")][..]),
            None,
            Some("https://*"),
            true,
            None,
        )],
    );
    check(
        &filter,
        &[mc(
            MATCH_CATEGORY_HOST,
            Some("android.intent.action.VIEW"),
            Some(&[Some("android.intent.category.BROWSABLE")][..]),
            None,
            Some("https://"),
            true,
            None,
        )],
    );
}

/// CTS `testAppEnumerationNoPortMatchesPortFilter`.
#[test]
fn app_enumeration_no_port_matches_port_filter() {
    let filter = Match::new(
        Some(&[Some("android.intent.action.VIEW")][..]),
        Some(&[Some("android.intent.category.BROWSABLE")][..]),
        None,
        Some(&[Some("http"), Some("https")][..]),
        Some(&[Some("*")][..]),
        Some(&[Some("81")][..]),
    );
    check(
        &filter,
        &[mc(
            MATCH_CATEGORY_HOST,
            Some("android.intent.action.VIEW"),
            Some(&[Some("android.intent.category.BROWSABLE")][..]),
            None,
            Some("https://something"),
            true,
            None,
        )],
    );
}

/// CTS `testAppEnumerationBrowser`.
#[test]
fn app_enumeration_browser() {
    let app_with_web_link = Match::new(
        Some(&[Some("android.intent.action.VIEW")][..]),
        Some(&[Some("android.intent.category.BROWSABLE")][..]),
        None,
        Some(&[Some("http"), Some("https")][..]),
        Some(&[Some("some.app.domain")][..]),
        None,
    );
    let app_with_wildcard_web_link = Match::new(
        Some(&[Some("android.intent.action.VIEW")][..]),
        Some(&[Some("android.intent.category.BROWSABLE")][..]),
        None,
        Some(&[Some("http"), Some("https")][..]),
        Some(&[Some("*.app.domain")][..]),
        None,
    );
    let browser_filter_with_wildcard = Match::new(
        Some(&[Some("android.intent.action.VIEW")][..]),
        Some(&[Some("android.intent.category.BROWSABLE")][..]),
        None,
        Some(&[Some("http"), Some("https")][..]),
        Some(&[Some("*")][..]),
        None,
    );
    let browser_filter_without_wildcard = Match::new(
        Some(&[Some("android.intent.action.VIEW")][..]),
        Some(&[Some("android.intent.category.BROWSABLE")][..]),
        None,
        Some(&[Some("http"), Some("https")][..]),
        None,
        None,
    );
    check(
        &browser_filter_with_wildcard,
        &[mc(
            MATCH_CATEGORY_HOST,
            Some("android.intent.action.VIEW"),
            Some(&[Some("android.intent.category.BROWSABLE")][..]),
            None,
            Some("https://"),
            true,
            None,
        )],
    );
    check(
        &browser_filter_without_wildcard,
        &[mc(
            MATCH_CATEGORY_SCHEME | MATCH_ADJUSTMENT_NORMAL,
            Some("android.intent.action.VIEW"),
            Some(&[Some("android.intent.category.BROWSABLE")][..]),
            None,
            Some("https://"),
            true,
            None,
        )],
    );
    check(
        &app_with_web_link,
        &[mc(
            NO_MATCH_DATA,
            Some("android.intent.action.VIEW"),
            Some(&[Some("android.intent.category.BROWSABLE")][..]),
            None,
            Some("https://"),
            true,
            None,
        )],
    );
    check(
        &app_with_wildcard_web_link,
        &[mc(
            NO_MATCH_DATA,
            Some("android.intent.action.VIEW"),
            Some(&[Some("android.intent.category.BROWSABLE")][..]),
            None,
            Some("https://"),
            true,
            None,
        )],
    );
}

/// CTS `testDataPaths`.
#[test]
fn data_paths() {
    let filter = Match::new(
        None,
        None,
        None,
        Some(&[Some("scheme1")][..]),
        Some(&[Some("authority1")][..]),
        Some(&[None][..]),
    );
    check(
        &filter,
        &[
            mc(NO_MATCH_DATA, None, None, None, None, false, None),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme1:foo"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_HOST,
                None,
                None,
                None,
                Some("scheme1://authority1/"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme1://authority2/"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_HOST,
                None,
                None,
                None,
                Some("scheme1://authority1:100/"),
                false,
                None,
            ),
        ],
    );
    let filter = Match::new(
        None,
        None,
        None,
        Some(&[Some("scheme1")][..]),
        Some(&[Some("authority1")][..]),
        Some(&[Some("100")][..]),
    );
    check(
        &filter,
        &[
            mc(NO_MATCH_DATA, None, None, None, None, false, None),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme1:foo"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme1://authority1/"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme1://authority2/"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_PORT,
                None,
                None,
                None,
                Some("scheme1://authority1:100/"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme1://authority1:200/"),
                false,
                None,
            ),
        ],
    );
    let filter = Match::new(
        None,
        None,
        None,
        Some(&[Some("scheme1")][..]),
        Some(&[Some("authority1"), Some("authority2")][..]),
        Some(&[Some("100"), None][..]),
    );
    check(
        &filter,
        &[
            mc(NO_MATCH_DATA, None, None, None, None, false, None),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme1:foo"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme1://authority1/"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_HOST,
                None,
                None,
                None,
                Some("scheme1://authority2/"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_PORT,
                None,
                None,
                None,
                Some("scheme1://authority1:100/"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme1://authority1:200/"),
                false,
                None,
            ),
        ],
    );
}

/// CTS `testPaths`.
#[test]
fn paths() {
    let filter = Match::new(
        None,
        None,
        None,
        Some(&[Some("scheme")][..]),
        Some(&[Some("authority")][..]),
        None,
    )
    .paths(
        Some(&[Some("/literal1"), Some("/2literal")][..]),
        Some(&[PATTERN_LITERAL, PATTERN_LITERAL][..]),
    );
    check(
        &filter,
        &[
            mc(NO_MATCH_DATA, None, None, None, None, false, None),
            mc(
                MATCH_CATEGORY_PATH,
                None,
                None,
                None,
                Some("scheme://authority/literal1"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_PATH,
                None,
                None,
                None,
                Some("scheme://authority/2literal"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme://authority/literal"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme://authority/literal12"),
                false,
                None,
            ),
        ],
    );
    let filter = Match::new(
        None,
        None,
        None,
        Some(&[Some("scheme")][..]),
        Some(&[Some("authority")][..]),
        None,
    )
    .paths(
        Some(&[Some("/literal1"), Some("/2literal")][..]),
        Some(&[PATTERN_PREFIX, PATTERN_PREFIX][..]),
    );
    check(
        &filter,
        &[
            mc(NO_MATCH_DATA, None, None, None, None, false, None),
            mc(
                MATCH_CATEGORY_PATH,
                None,
                None,
                None,
                Some("scheme://authority/literal1"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_PATH,
                None,
                None,
                None,
                Some("scheme://authority/2literal"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme://authority/literal"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_PATH,
                None,
                None,
                None,
                Some("scheme://authority/literal12"),
                false,
                None,
            ),
        ],
    );
    let filter = Match::new(
        None,
        None,
        None,
        Some(&[Some("scheme")][..]),
        Some(&[Some("authority")][..]),
        None,
    )
    .paths(
        Some(&[Some("literal1"), Some("2literal")][..]),
        Some(&[PATTERN_SUFFIX, PATTERN_SUFFIX][..]),
    );
    check(
        &filter,
        &[
            mc(NO_MATCH_DATA, None, None, None, None, false, None),
            mc(
                MATCH_CATEGORY_PATH,
                None,
                None,
                None,
                Some("scheme://authority/aliteral1"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_PATH,
                None,
                None,
                None,
                Some("scheme://authority/2literal"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme://authority/literal"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_PATH,
                None,
                None,
                None,
                Some("scheme://authority/literal1"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_PATH,
                None,
                None,
                None,
                Some("scheme://authority/2literal1"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme://authority/literal1a"),
                false,
                None,
            ),
        ],
    );
    let filter = Match::new(
        None,
        None,
        None,
        Some(&[Some("scheme")][..]),
        Some(&[Some("authority")][..]),
        None,
    )
    .paths(Some(&[Some("/.*")][..]), Some(&[PATTERN_SIMPLE_GLOB][..]));
    check(
        &filter,
        &[
            mc(NO_MATCH_DATA, None, None, None, None, false, None),
            mc(
                MATCH_CATEGORY_PATH,
                None,
                None,
                None,
                Some("scheme://authority/literal1"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_PATH,
                None,
                None,
                None,
                Some("scheme://authority/"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme://authority"),
                false,
                None,
            ),
        ],
    );
    let filter = Match::new(
        None,
        None,
        None,
        Some(&[Some("scheme")][..]),
        Some(&[Some("authority")][..]),
        None,
    )
    .paths(Some(&[Some(".*")][..]), Some(&[PATTERN_SIMPLE_GLOB][..]));
    check(
        &filter,
        &[
            mc(NO_MATCH_DATA, None, None, None, None, false, None),
            mc(
                MATCH_CATEGORY_PATH,
                None,
                None,
                None,
                Some("scheme://authority/literal1"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_PATH,
                None,
                None,
                None,
                Some("scheme://authority/"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_PATH,
                None,
                None,
                None,
                Some("scheme://authority"),
                false,
                None,
            ),
        ],
    );
    let filter = Match::new(
        None,
        None,
        None,
        Some(&[Some("scheme")][..]),
        Some(&[Some("authority")][..]),
        None,
    )
    .paths(Some(&[Some("/a1*b")][..]), Some(&[PATTERN_SIMPLE_GLOB][..]));
    check(
        &filter,
        &[
            mc(NO_MATCH_DATA, None, None, None, None, false, None),
            mc(
                MATCH_CATEGORY_PATH,
                None,
                None,
                None,
                Some("scheme://authority/ab"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_PATH,
                None,
                None,
                None,
                Some("scheme://authority/a1b"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_PATH,
                None,
                None,
                None,
                Some("scheme://authority/a11b"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme://authority/a2b"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme://authority/a1bc"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme://authority/"),
                false,
                None,
            ),
        ],
    );
    let filter = Match::new(
        None,
        None,
        None,
        Some(&[Some("scheme")][..]),
        Some(&[Some("authority")][..]),
        None,
    )
    .paths(Some(&[Some("/a1*")][..]), Some(&[PATTERN_SIMPLE_GLOB][..]));
    check(
        &filter,
        &[
            mc(NO_MATCH_DATA, None, None, None, None, false, None),
            mc(
                MATCH_CATEGORY_PATH,
                None,
                None,
                None,
                Some("scheme://authority/a1"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme://authority/ab"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_PATH,
                None,
                None,
                None,
                Some("scheme://authority/a11"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme://authority/a1b"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_PATH,
                None,
                None,
                None,
                Some("scheme://authority/a11"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme://authority/a2"),
                false,
                None,
            ),
        ],
    );
    let filter = Match::new(
        None,
        None,
        None,
        Some(&[Some("scheme")][..]),
        Some(&[Some("authority")][..]),
        None,
    )
    .paths(
        Some(&[Some("/a\\.*b")][..]),
        Some(&[PATTERN_SIMPLE_GLOB][..]),
    );
    check(
        &filter,
        &[
            mc(NO_MATCH_DATA, None, None, None, None, false, None),
            mc(
                MATCH_CATEGORY_PATH,
                None,
                None,
                None,
                Some("scheme://authority/ab"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_PATH,
                None,
                None,
                None,
                Some("scheme://authority/a.b"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_PATH,
                None,
                None,
                None,
                Some("scheme://authority/a..b"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme://authority/a2b"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme://authority/a.bc"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme://authority/"),
                false,
                None,
            ),
        ],
    );
    let filter = Match::new(
        None,
        None,
        None,
        Some(&[Some("scheme")][..]),
        Some(&[Some("authority")][..]),
        None,
    )
    .paths(Some(&[Some("/a.*b")][..]), Some(&[PATTERN_SIMPLE_GLOB][..]));
    check(
        &filter,
        &[
            mc(NO_MATCH_DATA, None, None, None, None, false, None),
            mc(
                MATCH_CATEGORY_PATH,
                None,
                None,
                None,
                Some("scheme://authority/ab"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_PATH,
                None,
                None,
                None,
                Some("scheme://authority/a.b"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_PATH,
                None,
                None,
                None,
                Some("scheme://authority/a.1b"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_PATH,
                None,
                None,
                None,
                Some("scheme://authority/a2b"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme://authority/a.bc"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme://authority/"),
                false,
                None,
            ),
        ],
    );
    let filter = Match::new(
        None,
        None,
        None,
        Some(&[Some("scheme")][..]),
        Some(&[Some("authority")][..]),
        None,
    )
    .paths(Some(&[Some("/a.*")][..]), Some(&[PATTERN_SIMPLE_GLOB][..]));
    check(
        &filter,
        &[
            mc(NO_MATCH_DATA, None, None, None, None, false, None),
            mc(
                MATCH_CATEGORY_PATH,
                None,
                None,
                None,
                Some("scheme://authority/ab"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_PATH,
                None,
                None,
                None,
                Some("scheme://authority/a.b"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_PATH,
                None,
                None,
                None,
                Some("scheme://authority/a.1b"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_PATH,
                None,
                None,
                None,
                Some("scheme://authority/a2b"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_PATH,
                None,
                None,
                None,
                Some("scheme://authority/a.bc"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme://authority/"),
                false,
                None,
            ),
        ],
    );
    let filter = Match::new(
        None,
        None,
        None,
        Some(&[Some("scheme")][..]),
        Some(&[Some("authority")][..]),
        None,
    )
    .paths(
        Some(&[Some("/a.\\*b")][..]),
        Some(&[PATTERN_SIMPLE_GLOB][..]),
    );
    check(
        &filter,
        &[
            mc(NO_MATCH_DATA, None, None, None, None, false, None),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme://authority/ab"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_PATH,
                None,
                None,
                None,
                Some("scheme://authority/a.*b"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_PATH,
                None,
                None,
                None,
                Some("scheme://authority/a1*b"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme://authority/a2b"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme://authority/a.bc"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme://authority/"),
                false,
                None,
            ),
        ],
    );
    let filter = Match::new(
        None,
        None,
        None,
        Some(&[Some("scheme")][..]),
        Some(&[Some("authority")][..]),
        None,
    )
    .paths(
        Some(&[Some("/a.\\*")][..]),
        Some(&[PATTERN_SIMPLE_GLOB][..]),
    );
    check(
        &filter,
        &[
            mc(NO_MATCH_DATA, None, None, None, None, false, None),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme://authority/ab"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_PATH,
                None,
                None,
                None,
                Some("scheme://authority/a.*"),
                false,
                None,
            ),
            mc(
                MATCH_CATEGORY_PATH,
                None,
                None,
                None,
                Some("scheme://authority/a1*"),
                false,
                None,
            ),
            mc(
                NO_MATCH_DATA,
                None,
                None,
                None,
                Some("scheme://authority/a1b"),
                false,
                None,
            ),
        ],
    );
}

/// `ParsedIntentInfoImpl.writeToParcel`'s form.
#[test]
fn reads_a_parsed_intent_info() {
    let mut p = Parcel::new();
    p.write_i32(0x5); // has default, has a label
    p.write_i32(7);
    p.write_i32(1);
    p.write_string8(Some("Label"));
    p.write_i32(9);
    p.write_i32(1); // the filter
    p.write_i32(1);
    p.write_string16(Some(ACTION));
    for _ in 0..5 {
        p.write_i32(0); // categories .. mime groups
    }
    for _ in 0..3 {
        p.write_i32(0); // scheme-specific parts, authorities, paths
    }
    for v in [-1, 0, 0, 0, 0, 0, 0, 0] {
        p.write_i32(v); // priority .. relative groups
    }
    let mut r = Reader::new(p.data(), &[]);
    let info = ParsedIntentInfo::read(&mut r, &mut Plain).unwrap();
    assert_eq!(r.remaining(), 0);
    assert!(info.has_default);
    assert_eq!((info.label_res, info.icon), (7, 9));
    assert_eq!(info.non_localized_label.as_deref(), Some("Label"));
    assert_eq!((info.filter.actions.len(), info.filter.priority), (1, -1));
}
