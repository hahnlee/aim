//! DomainVerificationUtils.isValidDomain for URI-group updates, distinct from
//! the collector's Unicode Patterns matcher. AOSP android-16.0.0_r1, ASL 2.0.
pub fn valid_domain(domain: &str) -> Result<bool, String> {
    if domain.encode_utf16().count() > 254 || domain == "*" {
        return Ok(false);
    }
    if domain.is_empty() {
        return Err("empty URI-group domain".into());
    }
    let domain = if domain.starts_with('*') {
        let Some(value) = domain.strip_prefix("*.") else {
            return Ok(false);
        };
        value
    } else {
        domain
    };
    let labels: Vec<_> = domain.split('.').collect();
    Ok(labels.len() > 1
        && labels.iter().all(|label| {
            (1..=63).contains(&label.len())
                && label
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-')
        }))
}
