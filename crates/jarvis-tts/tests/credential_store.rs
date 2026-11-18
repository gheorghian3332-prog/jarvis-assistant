//! The real Windows Credential Manager. Ignored by default.
//!
//! It stores a dummy text (not a key) under the ElevenLabs entry and removes it again. If a real key
//! is already stored the test does nothing, so it can never overwrite or delete one.
//!
//! Run with `cargo test -p jarvis-tts --test credential_store -- --ignored --nocapture`.

use jarvis_tts::{CredentialStore, SecretStore, SecretString, ELEVEN_KEY};

#[test]
#[ignore = "touches the Windows Credential Manager"]
fn credential_manager_round_trip() {
    let store = CredentialStore;
    if store.exists(ELEVEN_KEY).expect("exists") {
        println!("an ElevenLabs entry already exists: leaving it alone, test skipped");
        return;
    }
    store
        .set(ELEVEN_KEY, SecretString::new("dummy-text-not-a-key"))
        .expect("set");
    assert!(store.exists(ELEVEN_KEY).expect("exists"));
    let value = store.get(ELEVEN_KEY).expect("get").expect("value");
    assert_eq!(value.expose(), "dummy-text-not-a-key");
    assert!(!format!("{value:?}").contains("dummy-text"));
    store.clear(ELEVEN_KEY).expect("clear");
    assert!(!store.exists(ELEVEN_KEY).expect("exists"));
    store.clear(ELEVEN_KEY).expect("clearing again is fine");
    assert!(store.set("other_name", SecretString::new("x")).is_err());
    println!("credential manager: set / get / exists / clear all work");
}
