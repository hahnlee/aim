//! Original Settings verifier read/retry projection; identities are fixture data.
use aim_android_xml::{Element, Node, Value};
use aim_services::package::settings::{ReadError, Settings};

pub fn inputs() -> Vec<Vec<u8>> {
    let mut values = vec![
        None,
        Some(String::new()),
        Some("a00001111aaaa".into()),
        Some("--aaaaaaaaaaaaa--".into()),
        Some("p777777777777".into()),
        Some("QAAAAAAAAAAAA".into()),
        Some("AAAAAAAAAAAAAA".into()),
        Some("😀AAAAAAAAAAAA".into()),
    ];
    for offset in [0, 6, 12] {
        for byte in 0..128u8 {
            let mut identity = vec![b'A'; 13];
            identity[offset] = byte;
            values.push(Some(String::from_utf8(identity).unwrap()));
        }
    }
    values
        .into_iter()
        .map(|value| {
            let mut root = aim_android_xml::read(
                b"<packages><verifier device='AAAA-AAAA-AAAA-A'/></packages>",
            )
            .unwrap();
            root.content.push(Node::Element(Element {
                name: "verifier".into(),
                attrs: value
                    .into_iter()
                    .map(|value| ("device".into(), Value::String(value)))
                    .collect(),
                content: vec![],
            }));
            aim_android_xml::abx::write(&root).unwrap()
        })
        .collect()
}

pub fn read(bytes: &[u8]) -> String {
    let mut state = Settings::default();
    let result = state.read_document(bytes, |_, _, _| Ok(false));
    let status = if matches!(result, Err(ReadError::FatalInput(_))) {
        "fatal"
    } else {
        "ok"
    };
    // Caught input errors retry an empty reserve, leaving the prior identity.
    format!("{status}|{}", state.verifier.as_deref().unwrap_or("null"))
}
