use keyring::Entry;
use secrecy::SecretString;

pub fn resolve_key(raw_key: Option<String>) -> Option<SecretString> {
    let key = raw_key?;
    if key.starts_with("keystore://") {
        let identifier = key.strip_prefix("keystore://").unwrap();
        // Uses the service name "kezen" and the identifier as the username
        let entry = Entry::new("kezen", identifier).ok()?;
        match entry.get_password() {
            Ok(pw) => Some(SecretString::from(pw)),
            Err(e) => {
                tracing::warn!(error = %e, identifier = %identifier, "Failed to retrieve key from OS keychain");
                None
            }
        }
    } else {
        Some(SecretString::from(key))
    }
}
