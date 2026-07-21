// SpaceMolt Viewer — Credential store via IOTA Stronghold.
//
// Stores per-account passwords encrypted at rest using IOTA Stronghold
// (Decision #3 from grilling 2026-07-19). Cross-platform: works on WSL
// dev (Linux) and Windows release — no `#[cfg(target_os = ...)]` branching,
// no Windows Credential Manager, no DPAPI.
//
// The tauri-plugin-stronghold plugin is registered in lib.rs for
// frontend-initiated vault operations (load_store/create_store via IPC).
// This module uses iota_stronghold directly for Rust-side credential
// management — the plugin's internal StrongholdCollection is private
// and not accessible from our code, so we manage our own instance.
//
// Architecture:
//   - Stronghold snapshot file at `app_data_dir/credentials.stronghold`
//   - KeyProvider derived from user-supplied password (blake2b hash)
//   - Client per credential type (e.g. "accounts" client)
//   - Store (key-value) for username → password pairs
//   - All data encrypted at rest via XChaCha20-Poly1305
//
// Usage:
//   ```ignore
//   let mut store = CredentialStore::new("/path/to/credentials.stronghold");
//   store.initialize("master_password")?;  // create or load snapshot
//   store.save_password("stoleas", "hunter2")?;
//   let password = store.load_password("stoleas")?; // Some("hunter2")
//   store.delete_password("stoleas")?;
//   store.save()?; // persist to disk
//   ```

use iota_stronghold::{
    Client, KeyProvider, SnapshotPath, Stronghold,
};
use std::path::{Path, PathBuf};
use thiserror::Error;
use tracing::info;
use zeroize::Zeroizing;

/// Client name for account credentials within the Stronghold snapshot.
const ACCOUNTS_CLIENT: &[u8] = b"accounts";

/// Key prefix for stored passwords (in the Store key-value map).
fn cred_key(username: &str) -> Vec<u8> {
    format!("password:{username}").into_bytes()
}

/// Key for the list of stored account usernames.
const ACCOUNTS_LIST_KEY: &[u8] = b"accounts_list";

/// Errors from the credential store.
#[derive(Error, Debug)]
pub enum CredentialError {
    /// Stronghold not initialized — call `initialize()` first.
    #[error("credential store not initialized — call initialize() first")]
    NotInitialized,

    /// Failed to create or load the Stronghold snapshot.
    #[error("failed to initialize stronghold: {0}")]
    InitFailed(String),

    /// Failed to save/load/delete a credential.
    #[error("credential operation failed: {0}")]
    OperationFailed(String),

    /// Failed to persist the snapshot to disk.
    #[error("failed to save snapshot: {0}")]
    SaveFailed(String),

    /// Account not found.
    #[error("account '{0}' not found")]
    AccountNotFound(String),

    /// Password was empty or invalid.
    #[error("invalid credential: {0}")]
    InvalidCredential(String),
}

/// Credential store backed by IOTA Stronghold (encrypted at rest).
///
/// Manages its own `iota_stronghold::Stronghold` instance, independent
/// of the `tauri-plugin-stronghold` Tauri plugin. The Tauri plugin handles
/// frontend IPC (load_store/create_store); this module handles Rust-side
/// credential operations.
pub struct CredentialStore {
    /// Path to the Stronghold snapshot file.
    snapshot_path: PathBuf,
    /// The Stronghold instance. None until `initialize()` is called.
    stronghold: Option<Stronghold>,
    /// KeyProvider for snapshot encryption. Retained for `save()`.
    key_provider: Option<KeyProvider>,
}

impl CredentialStore {
    /// Create a new credential store pointing at a snapshot file.
    /// Does NOT initialize — call `initialize()` to create or load the
    /// encrypted snapshot.
    pub fn new<P: AsRef<Path>>(snapshot_path: P) -> Self {
        Self {
            snapshot_path: snapshot_path.as_ref().to_path_buf(),
            stronghold: None,
            key_provider: None,
        }
    }

    /// Initialize the credential store: create a new Stronghold instance,
    /// derive a key from the password, and either load an existing
    /// snapshot or create a fresh one.
    ///
    /// The password is hashed with blake2b256 to derive the encryption key.
    /// The same password must be supplied on subsequent loads.
    pub fn initialize(&mut self, password: &str) -> Result<(), CredentialError> {
        if password.is_empty() {
            return Err(CredentialError::InvalidCredential(
                "password must not be empty".into(),
            ));
        }

        // Derive key from password using blake2b256 (same approach as
        // tauri-plugin-stronghold's default, just using a different hash
        // function since we use the iota_stronghold API directly).
        let key_provider = KeyProvider::with_passphrase_hashed(
            Zeroizing::new(password.as_bytes().to_vec()),
            crypto::hashes::blake2b::Blake2b256::new(),
        )
        .map_err(|e| CredentialError::InitFailed(format!("key derivation failed: {e}")))?;

        let stronghold = Stronghold::default();

        let snapshot_path = SnapshotPath::from_path(&self.snapshot_path);
        if snapshot_path.exists() {
            // Load existing snapshot
            stronghold
                .load_snapshot(&key_provider, &snapshot_path)
                .map_err(|e| {
                    CredentialError::InitFailed(format!(
                        "failed to load snapshot (wrong password?): {e}"
                    ))
                })?;
            info!("Loaded existing credential snapshot");

            // Load the "accounts" client from the snapshot (not create —
            // create makes a fresh empty client, losing saved data).
            match stronghold.load_client(ACCOUNTS_CLIENT) {
                Ok(_) => info!("Loaded accounts client from snapshot"),
                Err(iota_stronghold::ClientError::ClientDataNotPresent) => {
                    // Client doesn't exist in snapshot yet — create it
                    info!("Accounts client not in snapshot, creating new");
                    stronghold
                        .create_client(ACCOUNTS_CLIENT)
                        .map_err(|e| CredentialError::InitFailed(format!("create_client failed: {e}")))?;
                }
                Err(e) => {
                    return Err(CredentialError::InitFailed(format!(
                        "load_client failed: {e}"
                    )))
                }
            }
        } else {
            // Fresh snapshot — create the client
            info!("Creating new credential snapshot");
            stronghold
                .create_client(ACCOUNTS_CLIENT)
                .map_err(|e| CredentialError::InitFailed(format!("create_client failed: {e}")))?;
        }

        self.stronghold = Some(stronghold);
        self.key_provider = Some(key_provider);
        Ok(())
    }

    /// Check if the store is initialized.
    pub fn is_initialized(&self) -> bool {
        self.stronghold.is_some()
    }

    /// Get the client for account credentials, or error if not initialized.
    fn client(&self) -> Result<Client, CredentialError> {
        let stronghold = self
            .stronghold
            .as_ref()
            .ok_or(CredentialError::NotInitialized)?;
        stronghold
            .get_client(ACCOUNTS_CLIENT)
            .map_err(|e| CredentialError::OperationFailed(format!("get_client failed: {e}")))
    }

    /// Save a password for an account. Overwrites if the account exists.
    pub fn save_password(&mut self, username: &str, password: &str) -> Result<(), CredentialError> {
        if username.is_empty() {
            return Err(CredentialError::InvalidCredential(
                "username must not be empty".into(),
            ));
        }
        if password.is_empty() {
            return Err(CredentialError::InvalidCredential(
                "password must not be empty".into(),
            ));
        }

        let client = self.client()?;
        let store = client.store();

        // Store the password
        store
            .insert(
                cred_key(username),
                password.as_bytes().to_vec(),
                None,
            )
            .map_err(|e| CredentialError::OperationFailed(format!("save password failed: {e}")))?;

        // Update the accounts list
        let mut accounts = self.list_accounts_internal()?;
        if !accounts.contains(&username.to_string()) {
            accounts.push(username.to_string());
            let accounts_json = serde_json::to_vec(&accounts)
                .map_err(|e| CredentialError::OperationFailed(format!("serialize accounts list: {e}")))?;
            store
                .insert(ACCOUNTS_LIST_KEY.to_vec(), accounts_json, None)
                .map_err(|e| CredentialError::OperationFailed(format!("save accounts list failed: {e}")))?;
        }

        Ok(())
    }

    /// Load a password for an account. Returns None if the account doesn't exist.
    pub fn load_password(&self, username: &str) -> Result<Option<String>, CredentialError> {
        let client = self.client()?;
        let store = client.store();

        let key = cred_key(username);
        let data = store
            .get(&key)
            .map_err(|e| CredentialError::OperationFailed(format!("load password failed: {e}")))?;

        match data {
            Some(bytes) => {
                let password = String::from_utf8(bytes)
                    .map_err(|e| CredentialError::OperationFailed(format!("password decode failed: {e}")))?;
                Ok(Some(password))
            }
            None => Ok(None),
        }
    }

    /// Delete a password for an account.
    pub fn delete_password(&mut self, username: &str) -> Result<(), CredentialError> {
        let client = self.client()?;
        let store = client.store();

        store
            .delete(&cred_key(username))
            .map_err(|e| CredentialError::OperationFailed(format!("delete password failed: {e}")))?;

        // Remove from accounts list
        let mut accounts = self.list_accounts_internal()?;
        accounts.retain(|a| a != username);
        let accounts_json = serde_json::to_vec(&accounts)
            .map_err(|e| CredentialError::OperationFailed(format!("serialize accounts list: {e}")))?;
        store
            .insert(ACCOUNTS_LIST_KEY.to_vec(), accounts_json, None)
            .map_err(|e| CredentialError::OperationFailed(format!("save accounts list failed: {e}")))?;

        Ok(())
    }

    /// List all stored account usernames.
    pub fn list_accounts(&self) -> Result<Vec<String>, CredentialError> {
        self.list_accounts_internal()
    }

    /// Check if an account exists.
    pub fn account_exists(&self, username: &str) -> Result<bool, CredentialError> {
        let client = self.client()?;
        let store = client.store();
        store
            .contains_key(&cred_key(username))
            .map_err(|e| CredentialError::OperationFailed(format!("account_exists check failed: {e}")))
    }

    /// Internal: list accounts from the store's accounts_list key.
    fn list_accounts_internal(&self) -> Result<Vec<String>, CredentialError> {
        let client = self.client()?;
        let store = client.store();

        let data = store
            .get(ACCOUNTS_LIST_KEY)
            .map_err(|e| CredentialError::OperationFailed(format!("load accounts list failed: {e}")))?;

        match data {
            Some(bytes) => {
                let accounts: Vec<String> = serde_json::from_slice(&bytes)
                    .map_err(|e| CredentialError::OperationFailed(format!("deserialize accounts list: {e}")))?;
                Ok(accounts)
            }
            None => Ok(Vec::new()),
        }
    }

    /// Persist the credential store to disk. Must be called after
    /// modifications for them to survive process restart.
    pub fn save(&self) -> Result<(), CredentialError> {
        let stronghold = self
            .stronghold
            .as_ref()
            .ok_or(CredentialError::NotInitialized)?;
        let key_provider = self
            .key_provider
            .as_ref()
            .ok_or(CredentialError::NotInitialized)?;

        let snapshot_path = SnapshotPath::from_path(&self.snapshot_path);

        stronghold
            .commit_with_keyprovider(&snapshot_path, key_provider)
            .map_err(|e| CredentialError::SaveFailed(format!("commit failed: {e}")))?;

        info!("Credential snapshot saved");
        Ok(())
    }

    /// Check if a snapshot file exists at the configured path.
    pub fn snapshot_exists(&self) -> bool {
        self.snapshot_path.exists()
    }

    /// Get the snapshot file path.
    pub fn snapshot_path(&self) -> &Path {
        &self.snapshot_path
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Helper: create a temp snapshot path.
    fn temp_snapshot() -> PathBuf {
        let dir = std::env::temp_dir();
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        dir.join(format!("spacemolt_test_{}.stronghold", nonce))
    }

    /// Helper: create an initialized credential store at a temp path.
    fn test_store() -> (CredentialStore, PathBuf) {
        let path = temp_snapshot();
        let mut store = CredentialStore::new(&path);
        store.initialize("test_master_password").unwrap();
        (store, path)
    }

    /// Helper: clean up snapshot file.
    fn cleanup(path: &Path) {
        let _ = fs::remove_file(path);
    }

    #[test]
    fn test_new_store_not_initialized() {
        let path = temp_snapshot();
        let store = CredentialStore::new(&path);
        assert!(!store.is_initialized());
        assert!(!store.snapshot_exists());
        cleanup(&path);
    }

    #[test]
    fn test_initialize_creates_store() {
        let path = temp_snapshot();
        let mut store = CredentialStore::new(&path);
        assert!(!store.is_initialized());

        store.initialize("mypassword").unwrap();
        assert!(store.is_initialized());
        cleanup(&path);
    }

    #[test]
    fn test_initialize_rejects_empty_password() {
        let path = temp_snapshot();
        let mut store = CredentialStore::new(&path);
        let result = store.initialize("");
        assert!(result.is_err());
        cleanup(&path);
    }

    #[test]
    fn test_save_and_load_password() {
        let (mut store, path) = test_store();

        store.save_password("stoleas", "hunter2").unwrap();
        let loaded = store.load_password("stoleas").unwrap();
        assert_eq!(loaded, Some("hunter2".to_string()));

        store.save().unwrap();
        cleanup(&path);
    }

    #[test]
    fn test_load_nonexistent_account() {
        let (store, path) = test_store();

        let loaded = store.load_password("nonexistent").unwrap();
        assert_eq!(loaded, None);

        cleanup(&path);
    }

    #[test]
    fn test_delete_password() {
        let (mut store, path) = test_store();

        store.save_password("testuser", "pass123").unwrap();
        assert!(store.account_exists("testuser").unwrap());

        store.delete_password("testuser").unwrap();
        assert!(!store.account_exists("testuser").unwrap());
        assert_eq!(store.load_password("testuser").unwrap(), None);

        cleanup(&path);
    }

    #[test]
    fn test_list_accounts() {
        let (mut store, path) = test_store();

        store.save_password("stoleas", "pass1").unwrap();
        store.save_password("atervisus", "pass2").unwrap();
        store.save_password("zh0ul", "pass3").unwrap();

        let accounts = store.list_accounts().unwrap();
        assert_eq!(accounts.len(), 3);
        assert!(accounts.contains(&"stoleas".to_string()));
        assert!(accounts.contains(&"atervisus".to_string()));
        assert!(accounts.contains(&"zh0ul".to_string()));

        cleanup(&path);
    }

    #[test]
    fn test_list_accounts_empty() {
        let (store, path) = test_store();
        let accounts = store.list_accounts().unwrap();
        assert!(accounts.is_empty());
        cleanup(&path);
    }

    #[test]
    fn test_save_overwrites_existing() {
        let (mut store, path) = test_store();

        store.save_password("user", "old_pass").unwrap();
        store.save_password("user", "new_pass").unwrap();

        let loaded = store.load_password("user").unwrap();
        assert_eq!(loaded, Some("new_pass".to_string()));

        // Should still be only 1 account
        let accounts = store.list_accounts().unwrap();
        assert_eq!(accounts.len(), 1);

        cleanup(&path);
    }

    #[test]
    fn test_delete_removes_from_list() {
        let (mut store, path) = test_store();

        store.save_password("user1", "pass1").unwrap();
        store.save_password("user2", "pass2").unwrap();

        store.delete_password("user1").unwrap();

        let accounts = store.list_accounts().unwrap();
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0], "user2");

        cleanup(&path);
    }

    #[test]
    fn test_save_rejects_empty_username() {
        let (mut store, path) = test_store();
        let result = store.save_password("", "password");
        assert!(result.is_err());
        cleanup(&path);
    }

    #[test]
    fn test_save_rejects_empty_password() {
        let (mut store, path) = test_store();
        let result = store.save_password("user", "");
        assert!(result.is_err());
        cleanup(&path);
    }

    #[test]
    fn test_persist_and_reload() {
        let path = temp_snapshot();

        // Create store, save a credential, persist to disk
        {
            let mut store = CredentialStore::new(&path);
            store.initialize("master_pass").unwrap();
            store.save_password("stoleas", "secret123").unwrap();
            store.save().unwrap();
        }

        // Reopen with same password — should load the saved credential
        {
            let mut store = CredentialStore::new(&path);
            store.initialize("master_pass").unwrap();
            let loaded = store.load_password("stoleas").unwrap();
            assert_eq!(loaded, Some("secret123".to_string()));

            let accounts = store.list_accounts().unwrap();
            assert_eq!(accounts.len(), 1);
            assert_eq!(accounts[0], "stoleas");
        }

        cleanup(&path);
    }

    #[test]
    fn test_reload_with_wrong_password_fails() {
        let path = temp_snapshot();

        // Create with one password
        {
            let mut store = CredentialStore::new(&path);
            store.initialize("correct_password").unwrap();
            store.save_password("user", "pass").unwrap();
            store.save().unwrap();
        }

        // Try to load with wrong password — should fail
        {
            let mut store = CredentialStore::new(&path);
            let result = store.initialize("wrong_password");
            assert!(result.is_err());
        }

        cleanup(&path);
    }

    #[test]
    fn test_operations_without_init_fail() {
        let path = temp_snapshot();
        let store = CredentialStore::new(&path);

        // All operations should fail with NotInitialized
        let result = store.load_password("user");
        assert!(result.is_err());

        let result = store.list_accounts();
        assert!(result.is_err());

        let result = store.account_exists("user");
        assert!(result.is_err());

        cleanup(&path);
    }

    #[test]
    fn test_snapshot_exists_check() {
        let path = temp_snapshot();
        let store = CredentialStore::new(&path);
        assert!(!store.snapshot_exists());

        {
            let mut store = CredentialStore::new(&path);
            store.initialize("pass").unwrap();
            store.save().unwrap();
        }

        // After save, snapshot file should exist
        let store = CredentialStore::new(&path);
        assert!(store.snapshot_exists());

        cleanup(&path);
    }

    #[test]
    fn test_snapshot_path_accessor() {
        let path = temp_snapshot();
        let store = CredentialStore::new(&path);
        assert_eq!(store.snapshot_path(), path);
        cleanup(&path);
    }
}