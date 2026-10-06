//! Legacy SettingsXml cursor/read-write projections against the original owner.
use aim_android_xml::pull::{Event, Reader};
use aim_services::package::domain_verification::State;

pub fn inputs() -> Vec<Vec<u8>> {
    let documents = [
        "<domain-verifications-legacy><user-states packageName='p'><user-state userId='3' state='2'/><user-state userId='1' state='4'/><user-state userId='3' state='5'/></user-states></domain-verifications-legacy>",
        "<domain-verifications-legacy><unknown><user-states packageName='p'><unknown><user-state userId='3' state='2'/></unknown></user-states></unknown></domain-verifications-legacy>",
        "<domain-verifications-legacy><user-states><user-state/><user-state userId='bad' state='bad'/></user-states><user-states packageName=''><user-state userId='-2' state='3'/></user-states></domain-verifications-legacy>",
        "<domain-verifications-legacy><user-states packageName='p'/><user-states packageName='p'><user-state userId='1' state='2'/></user-states></domain-verifications-legacy>",
        "<domain-verifications-legacy><user-states packageName='p'><user-state userId='1' state='2'/><",
        "<domain-verifications-legacy><user-states packageName='p'><user-state userId='1' state='2'></wrong>",
        "<domain-verifications-legacy><user-states packageName='p'><user-state userId='1' state='2'/></user-states></domain-verifications-legacy>broken",
    ];
    let mut out = Vec::new();
    for input in documents {
        out.push(input.as_bytes().to_vec());
        if let Ok(root) = aim_android_xml::read(input.as_bytes()) {
            out.push(aim_android_xml::abx::write(&root).unwrap());
        }
    }
    let binary =
        aim_android_xml::abx::write(&aim_android_xml::read(documents[0].as_bytes()).unwrap())
            .unwrap();
    for end in 1..binary.len() {
        if let Ok(mut reader) = Reader::new(&binary[..end]) {
            if matches!(reader.next(), Ok(Event::Start(_))) {
                out.push(binary[..end].to_vec());
            }
        }
    }
    out
}

pub fn read(bytes: &[u8]) -> State {
    let mut reader = Reader::with_document(bytes).unwrap();
    reader.next().unwrap();
    let mut state = State::default();
    let _diagnostics = state.read_legacy_events(&mut reader);
    state
}

pub fn projection(state: &State) -> String {
    let mut rows = state
        .legacy
        .iter()
        .filter(|(_, users)| !users.is_empty())
        .map(|(name, users)| {
            let mut users = users.clone();
            users.sort_by_key(|(id, _)| *id);
            format!(
                "{}:{}",
                name.as_deref().unwrap_or("<null>"),
                users
                    .iter()
                    .map(|(id, state)| format!("{id}={state}"))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        })
        .collect::<Vec<_>>();
    rows.sort();
    rows.join(";")
}
