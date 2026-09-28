use super::*;

pub(crate) fn replace_required(source: &mut String, from: &str, to: &str) -> Result<()> {
    if !source.contains(from) {
        return Err(
            format!("locked ART source no longer contains expected fragment: {from}").into(),
        );
    }
    *source = source.replacen(from, to, 1);
    Ok(())
}

pub(crate) fn read_lock(root: &Path) -> Result<BTreeMap<String, String>> {
    read_key_value_file(&root.join("sources.lock"))
}

pub(crate) fn read_key_value_file(path: &Path) -> Result<BTreeMap<String, String>> {
    let mut values = BTreeMap::new();
    for line in fs::read_to_string(path)?.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| format!("invalid line in {}: {line}", path.display()))?;
        values.insert(key.to_owned(), value.to_owned());
    }
    Ok(values)
}

pub(crate) fn lock_value<'a>(lock: &'a BTreeMap<String, String>, key: &str) -> Result<&'a str> {
    lock.get(key)
        .map(String::as_str)
        .ok_or_else(|| format!("missing {key} in lock file").into())
}

pub(crate) fn verify_sha256(path: &Path, expected: &str) -> Result<()> {
    let output = command_output(Command::new("shasum").args(["-a", "256"]).arg(path))?;
    let actual = output.split_whitespace().next().unwrap_or_default();
    if actual != expected {
        return Err(format!(
            "SHA-256 mismatch for {}: expected {expected}, found {actual}",
            path.display()
        )
        .into());
    }
    Ok(())
}
