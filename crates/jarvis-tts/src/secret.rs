//! Secrets (the ElevenLabs key) live only in the Windows Credential Manager.
//!
//! A [`SecretString`] never shows its value through `Debug` or `Serialize`; the value leaves it only
//! through [`SecretString::expose`], which is used for the HTTP header and nothing else.

use crate::error::{TtsError, TtsResult};
use serde::{Serialize, Serializer};
use std::{
    collections::HashMap,
    fmt,
    sync::{Mutex, MutexGuard},
};

/// Name of the ElevenLabs API key in the credential store.
pub const ELEVEN_KEY: &str = "elevenlabs_api_key";
/// Name of the optional cloud language model key (OpenAI-compatible endpoint).
pub const AI_CLOUD_KEY: &str = "ai_cloud_api_key";
/// Names `secret_set` / `secret_clear` / `secret_exists` accept.
pub const SECRET_NAMES: [&str; 2] = [ELEVEN_KEY, AI_CLOUD_KEY];
/// Upper bound for a secret value.
pub const MAX_SECRET_LEN: usize = 512;
const SERVICE: &str = "Jarvis";

/// Returns the name if it is one of the known secrets.
pub fn validate_name(name: &str) -> TtsResult<&'static str> {
    SECRET_NAMES
        .iter()
        .copied()
        .find(|known| *known == name)
        .ok_or_else(|| TtsError::Invalid(format!("unknown secret name `{name}`")))
}

/// A secret value; hidden from `Debug`, `Display` and serialization.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretString(String);

impl SecretString {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The raw value. Only for sending it to the service it belongs to.
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretString(***)")
    }
}

impl Serialize for SecretString {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str("***")
    }
}

/// Where secrets are kept.
pub trait SecretStore: Send + Sync {
    fn set(&self, name: &str, value: SecretString) -> TtsResult<()>;
    fn get(&self, name: &str) -> TtsResult<Option<SecretString>>;
    fn clear(&self, name: &str) -> TtsResult<()>;

    fn exists(&self, name: &str) -> TtsResult<bool> {
        Ok(self.get(name)?.is_some())
    }
}

/// Windows Credential Manager (generic credentials, target `Jarvis` / `<name>`).
#[derive(Debug, Default, Clone, Copy)]
pub struct CredentialStore;

impl CredentialStore {
    fn entry(name: &str) -> TtsResult<keyring::Entry> {
        let name = validate_name(name)?;
        keyring::Entry::new(SERVICE, name).map_err(|error| TtsError::Secret(error.to_string()))
    }
}

impl SecretStore for CredentialStore {
    fn set(&self, name: &str, value: SecretString) -> TtsResult<()> {
        if value.is_empty() || value.expose().len() > MAX_SECRET_LEN {
            return Err(TtsError::Invalid(format!(
                "a secret must be 1 to {MAX_SECRET_LEN} characters"
            )));
        }
        Self::entry(name)?
            .set_password(value.expose())
            .map_err(|error| TtsError::Secret(error.to_string()))
    }

    fn get(&self, name: &str) -> TtsResult<Option<SecretString>> {
        match Self::entry(name)?.get_password() {
            Ok(value) => Ok(Some(SecretString::new(value))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(TtsError::Secret(error.to_string())),
        }
    }

    fn clear(&self, name: &str) -> TtsResult<()> {
        match Self::entry(name)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(TtsError::Secret(error.to_string())),
        }
    }
}

/// In-memory store for tests.
#[derive(Default)]
pub struct MemoryStore {
    values: Mutex<HashMap<String, SecretString>>,
}

impl MemoryStore {
    fn values(&self) -> MutexGuard<'_, HashMap<String, SecretString>> {
        self.values
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl SecretStore for MemoryStore {
    fn set(&self, name: &str, value: SecretString) -> TtsResult<()> {
        let name = validate_name(name)?;
        if value.is_empty() || value.expose().len() > MAX_SECRET_LEN {
            return Err(TtsError::Invalid(
                "a secret must be 1 to 512 characters".into(),
            ));
        }
        self.values().insert(name.to_owned(), value);
        Ok(())
    }

    fn get(&self, name: &str) -> TtsResult<Option<SecretString>> {
        Ok(self.values().get(validate_name(name)?).cloned())
    }

    fn clear(&self, name: &str) -> TtsResult<()> {
        self.values().remove(validate_name(name)?);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAKE: &str = "not-a-real-key-0123456789";

    #[test]
    fn debug_and_serialize_never_show_the_value() {
        let secret = SecretString::new(FAKE);
        assert!(!format!("{secret:?}").contains(FAKE));
        assert!(!format!("{secret:#?}").contains(FAKE));
        let json = serde_json::to_string(&secret).expect("serialize");
        assert!(!json.contains(FAKE), "{json}");
        let wrapped = serde_json::json!({ "key": secret });
        assert!(!wrapped.to_string().contains(FAKE));
        assert_eq!(secret.expose(), FAKE);
    }

    #[test]
    fn errors_do_not_echo_the_value() {
        let store = MemoryStore::default();
        let error = store
            .set("no_such_secret", SecretString::new(FAKE))
            .expect_err("unknown name");
        assert!(!error.to_string().contains(FAKE));
        let error = store
            .set(
                ELEVEN_KEY,
                SecretString::new("x".repeat(MAX_SECRET_LEN + 1)),
            )
            .expect_err("too long");
        assert!(!format!("{error:?}").contains("xxxx"));
    }

    #[test]
    fn memory_store_sets_reads_and_clears() {
        let store = MemoryStore::default();
        assert!(!store.exists(ELEVEN_KEY).expect("exists"));
        store.set(ELEVEN_KEY, SecretString::new(FAKE)).expect("set");
        assert!(store.exists(ELEVEN_KEY).expect("exists"));
        assert_eq!(
            store.get(ELEVEN_KEY).expect("get").expect("value").expose(),
            FAKE
        );
        store.clear(ELEVEN_KEY).expect("clear");
        assert!(!store.exists(ELEVEN_KEY).expect("exists"));
        store.clear(ELEVEN_KEY).expect("clearing twice is fine");
    }

    #[test]
    fn only_known_names_are_accepted() {
        assert_eq!(
            validate_name("elevenlabs_api_key").expect("known"),
            ELEVEN_KEY
        );
        assert!(validate_name("password").is_err());
        assert!(validate_name("").is_err());
        let store = MemoryStore::default();
        assert!(store.exists("anything").is_err());
        assert!(CredentialStore.exists("anything").is_err());
    }
}
