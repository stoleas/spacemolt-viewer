// SpaceMolt Viewer — Account manager (multi-account persistence).
//
// Manages the account list (display name + username) as a JSON config file,
// with passwords stored separately in the encrypted CredentialStore (Stronghold).
// The account metadata (display names) is not sensitive — it's the passwords
// that need encryption. This separation keeps the Stronghold operations
// (slow due to Argon2-style key derivation) to a minimum: only load/save
// passwords go through Stronghold, not the full account list enumeration.
//
// The config file lives at a platform-appropriate app data path (set by
// the caller — typically `app_data_dir/accounts.json`). Passwords live
// in the Stronghold snapshot (encrypted at rest).
//
// Serial connection staggering (Decision #10): The AccountManager itself
// doesn't manage connections — that's Task 8 (AccountSession). But it
// provides the account list in a stable order for the serial connect queue.

use crate::credential_store::{CredentialError, CredentialStore};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use thiserror::Error;
use tracing::info;

/// Account metadata (non-sensitive). Stored in accounts.json.
/// Passwords are stored separately in the encrypted CredentialStore.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct AccountInfo {
    /// User-facing display name (e.g. "Drift", "Stoleas")
    pub display_name: String,
    /// Game login username (e.g. "stoleas", "atervisus")
    pub username: String,
}

/// Errors from the account manager.
#[derive(Error, Debug)]
pub enum AccountManagerError {
    /// Credential store error (Stronghold operation failed).
    #[error("credential error: {0}")]
    Credential(#[from] CredentialError),

    /// I/O error reading/writing the accounts config file.
    #[error("config I/O error: {0}")]
    Io(String),

    /// JSON serialization/deserialization error.
    #[error("config JSON error: {0}")]
    Json(String),

    /// Account already exists.
    #[error("account '{0}' already exists")]
    AccountExists(String),

    /// Account not found.
    #[error("account '{0}' not found")]
    AccountNotFound(String),

    /// Credential store not initialized.
    #[error("credential store not initialized — call init_credentials() first")]
    CredentialsNotInitialized,
}

/// Manages the account list with passwords backed by CredentialStore.
///
/// The account metadata (display_name + username) is stored as a simple
/// JSON file. Passwords are stored encrypted in the CredentialStore.
///
/// Usage:
///   ```ignore
///   let cred = CredentialStore::new("/path/to/credentials.stronghold");
///   let mut mgr = AccountManager::new(cred, PathBuf::from("/path/to/accounts.json"));
///   mgr.init_credentials("master_password")?; // initialize Stronghold
///   mgr.load().await?; // load accounts.json
///   mgr.add_account("Drift", "drift-user", "pass123")?;
///   mgr.save().await?; // persist accounts.json
///   mgr.save_credentials()?; // persist Stronghold snapshot
///   ```
pub struct AccountManager {
    /// Encrypted credential store for passwords.
    credential_store: CredentialStore,
    /// Path to the accounts.json config file.
    config_path: PathBuf,
    /// In-memory account list (display metadata only).
    pub accounts: Vec<AccountInfo>,
}

impl AccountManager {
    /// Create a new account manager.
    ///
    /// `credential_store` should be created but NOT yet initialized —
    /// call `init_credentials()` to set the Stronghold master password.
    pub fn new(credential_store: CredentialStore, config_path: PathBuf) -> Self {
        Self {
            credential_store,
            config_path,
            accounts: Vec::new(),
        }
    }

    /// Initialize the credential store with the master password.
    /// Must be called before any password operations.
    pub fn init_credentials(&mut self, password: &str) -> Result<(), AccountManagerError> {
        self.credential_store
            .initialize(password)
            .map_err(AccountManagerError::Credential)
    }

    /// Check if the credential store is initialized.
    pub fn credentials_initialized(&self) -> bool {
        self.credential_store.is_initialized()
    }

    /// Load the accounts list from the JSON config file.
    /// If the file doesn't exist, starts with an empty list.
    pub async fn load(&mut self) -> Result<(), AccountManagerError> {
        if !self.config_path.exists() {
            info!("No accounts config file found, starting fresh");
            return Ok(());
        }

        let json = tokio::fs::read_to_string(&self.config_path)
            .await
            .map_err(|e| AccountManagerError::Io(format!("read accounts.json: {e}")))?;

        self.accounts = serde_json::from_str(&json)
            .map_err(|e| AccountManagerError::Json(format!("parse accounts.json: {e}")))?;

        info!(count = self.accounts.len(), "Loaded accounts");
        Ok(())
    }

    /// Save the accounts list to the JSON config file.
    pub async fn save(&self) -> Result<(), AccountManagerError> {
        if let Some(parent) = self.config_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| AccountManagerError::Io(format!("create config dir: {e}")))?;
        }

        let json = serde_json::to_string_pretty(&self.accounts)
            .map_err(|e| AccountManagerError::Json(format!("serialize accounts: {e}")))?;

        tokio::fs::write(&self.config_path, json)
            .await
            .map_err(|e| AccountManagerError::Io(format!("write accounts.json: {e}")))?;

        info!(count = self.accounts.len(), "Saved accounts config");
        Ok(())
    }

    /// Persist the credential store (Stronghold snapshot) to disk.
    /// Call after password changes for them to survive restart.
    pub fn save_credentials(&self) -> Result<(), AccountManagerError> {
        self.credential_store
            .save()
            .map_err(AccountManagerError::Credential)
    }

    /// Add an account. Stores the password in the encrypted CredentialStore
    /// and adds the metadata to the in-memory list. Call `save()` and
    /// `save_credentials()` to persist.
    pub fn add_account(
        &mut self,
        display_name: &str,
        username: &str,
        password: &str,
    ) -> Result<(), AccountManagerError> {
        if !self.credentials_initialized() {
            return Err(AccountManagerError::CredentialsNotInitialized);
        }

        if self.accounts.iter().any(|a| a.username == username) {
            return Err(AccountManagerError::AccountExists(username.to_string()));
        }

        self.credential_store
            .save_password(username, password)
            .map_err(AccountManagerError::Credential)?;

        self.accounts.push(AccountInfo {
            display_name: display_name.to_string(),
            username: username.to_string(),
        });

        info!(username, display_name, "Added account");
        Ok(())
    }

    /// Remove an account. Deletes the password from the CredentialStore
    /// and removes the metadata from the in-memory list. Call `save()` and
    /// `save_credentials()` to persist.
    pub fn remove_account(&mut self, username: &str) -> Result<(), AccountManagerError> {
        if !self.credentials_initialized() {
            return Err(AccountManagerError::CredentialsNotInitialized);
        }

        if !self.accounts.iter().any(|a| a.username == username) {
            return Err(AccountManagerError::AccountNotFound(username.to_string()));
        }

        self.credential_store
            .delete_password(username)
            .map_err(AccountManagerError::Credential)?;

        self.accounts.retain(|a| a.username != username);

        info!(username, "Removed account");
        Ok(())
    }

    /// Get the password for an account.
    pub fn get_password(&self, username: &str) -> Result<Option<String>, AccountManagerError> {
        if !self.credentials_initialized() {
            return Err(AccountManagerError::CredentialsNotInitialized);
        }
        self.credential_store
            .load_password(username)
            .map_err(AccountManagerError::Credential)
    }

    /// Update the password for an existing account.
    pub fn update_password(
        &mut self,
        username: &str,
        new_password: &str,
    ) -> Result<(), AccountManagerError> {
        if !self.credentials_initialized() {
            return Err(AccountManagerError::CredentialsNotInitialized);
        }

        if !self.accounts.iter().any(|a| a.username == username) {
            return Err(AccountManagerError::AccountNotFound(username.to_string()));
        }

        self.credential_store
            .save_password(username, new_password)
            .map_err(AccountManagerError::Credential)?;

        info!(username, "Updated password");
        Ok(())
    }

    /// Update the display name for an existing account.
    pub fn update_display_name(
        &mut self,
        username: &str,
        new_display_name: &str,
    ) -> Result<(), AccountManagerError> {
        let account = self
            .accounts
            .iter_mut()
            .find(|a| a.username == username)
            .ok_or_else(|| AccountManagerError::AccountNotFound(username.to_string()))?;

        account.display_name = new_display_name.to_string();
        info!(username, display_name = new_display_name, "Updated display name");
        Ok(())
    }

    /// Get account info by username.
    pub fn get_account(&self, username: &str) -> Option<&AccountInfo> {
        self.accounts.iter().find(|a| a.username == username)
    }

    /// Check if an account exists.
    pub fn account_exists(&self, username: &str) -> bool {
        self.accounts.iter().any(|a| a.username == username)
    }

    /// List all accounts (display metadata only — no passwords).
    pub fn list_accounts(&self) -> &[AccountInfo] {
        &self.accounts
    }

    /// Get the number of accounts.
    pub fn account_count(&self) -> usize {
        self.accounts.len()
    }

    /// Get the config file path.
    pub fn config_path(&self) -> &std::path::Path {
        &self.config_path
    }

    /// Get a reference to the credential store.
    pub fn credential_store(&self) -> &CredentialStore {
        &self.credential_store
    }

    /// Get a mutable reference to the credential store.
    pub fn credential_store_mut(&mut self) -> &mut CredentialStore {
        &mut self.credential_store
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Helper: create a temp snapshot path for the credential store.
    fn temp_snapshot() -> PathBuf {
        let dir = std::env::temp_dir();
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        dir.join(format!("spacemolt_acct_test_{}.stronghold", nonce))
    }

    /// Helper: create a temp config path for accounts.json.
    fn temp_config() -> PathBuf {
        let dir = std::env::temp_dir();
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        dir.join(format!("spacemolt_acct_test_{}.json", nonce))
    }

    /// Helper: create an initialized account manager.
    fn test_manager() -> (AccountManager, PathBuf, PathBuf) {
        let snap = temp_snapshot();
        let cfg = temp_config();
        let cred = CredentialStore::new(&snap);
        let mut mgr = AccountManager::new(cred, cfg.clone());
        mgr.init_credentials("test_master_pass").unwrap();
        (mgr, snap, cfg)
    }

    /// Helper: clean up temp files.
    fn cleanup(snap: &PathBuf, cfg: &PathBuf) {
        let _ = std::fs::remove_file(snap);
        let _ = std::fs::remove_file(cfg);
    }

    #[test]
    fn test_new_manager_empty() {
        let snap = temp_snapshot();
        let cfg = temp_config();
        let cred = CredentialStore::new(&snap);
        let mgr = AccountManager::new(cred, cfg.clone());

        assert!(mgr.accounts.is_empty());
        assert!(!mgr.credentials_initialized());
        assert_eq!(mgr.account_count(), 0);
        assert_eq!(mgr.config_path(), cfg);

        cleanup(&snap, &cfg);
    }

    #[test]
    fn test_init_credentials() {
        let snap = temp_snapshot();
        let cfg = temp_config();
        let cred = CredentialStore::new(&snap);
        let mut mgr = AccountManager::new(cred, cfg.clone());

        assert!(!mgr.credentials_initialized());
        mgr.init_credentials("master_pass").unwrap();
        assert!(mgr.credentials_initialized());

        cleanup(&snap, &cfg);
    }

    #[test]
    fn test_add_account() {
        let (mut mgr, snap, cfg) = test_manager();

        mgr.add_account("Drift", "drift-user", "pass123").unwrap();
        assert_eq!(mgr.account_count(), 1);

        let acct = mgr.get_account("drift-user").unwrap();
        assert_eq!(acct.display_name, "Drift");
        assert_eq!(acct.username, "drift-user");

        cleanup(&snap, &cfg);
    }

    #[test]
    fn test_add_duplicate_fails() {
        let (mut mgr, snap, cfg) = test_manager();

        mgr.add_account("Drift", "drift-user", "pass123").unwrap();
        let result = mgr.add_account("Drift2", "drift-user", "pass456");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            AccountManagerError::AccountExists(_)
        ));

        cleanup(&snap, &cfg);
    }

    #[test]
    fn test_add_without_init_fails() {
        let snap = temp_snapshot();
        let cfg = temp_config();
        let cred = CredentialStore::new(&snap);
        let mut mgr = AccountManager::new(cred, cfg.clone());

        let result = mgr.add_account("Drift", "drift-user", "pass");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            AccountManagerError::CredentialsNotInitialized
        ));

        cleanup(&snap, &cfg);
    }

    #[test]
    fn test_get_password() {
        let (mut mgr, snap, cfg) = test_manager();

        mgr.add_account("Drift", "drift-user", "s3cr3t").unwrap();
        let password = mgr.get_password("drift-user").unwrap();
        assert_eq!(password, Some("s3cr3t".to_string()));

        cleanup(&snap, &cfg);
    }

    #[test]
    fn test_get_password_nonexistent() {
        let (mgr, snap, cfg) = test_manager();

        let password = mgr.get_password("nonexistent").unwrap();
        assert_eq!(password, None);

        cleanup(&snap, &cfg);
    }

    #[test]
    fn test_remove_account() {
        let (mut mgr, snap, cfg) = test_manager();

        mgr.add_account("Drift", "drift-user", "pass").unwrap();
        assert_eq!(mgr.account_count(), 1);

        mgr.remove_account("drift-user").unwrap();
        assert_eq!(mgr.account_count(), 0);
        assert!(!mgr.account_exists("drift-user"));

        // Password should be gone too
        let password = mgr.get_password("drift-user").unwrap();
        assert_eq!(password, None);

        cleanup(&snap, &cfg);
    }

    #[test]
    fn test_remove_nonexistent_fails() {
        let (mut mgr, snap, cfg) = test_manager();

        let result = mgr.remove_account("ghost");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            AccountManagerError::AccountNotFound(_)
        ));

        cleanup(&snap, &cfg);
    }

    #[test]
    fn test_update_password() {
        let (mut mgr, snap, cfg) = test_manager();

        mgr.add_account("Drift", "drift-user", "old_pass").unwrap();
        mgr.update_password("drift-user", "new_pass").unwrap();

        let password = mgr.get_password("drift-user").unwrap();
        assert_eq!(password, Some("new_pass".to_string()));

        cleanup(&snap, &cfg);
    }

    #[test]
    fn test_update_display_name() {
        let (mut mgr, snap, cfg) = test_manager();

        mgr.add_account("OldName", "drift-user", "pass").unwrap();
        mgr.update_display_name("drift-user", "NewName").unwrap();

        let acct = mgr.get_account("drift-user").unwrap();
        assert_eq!(acct.display_name, "NewName");

        cleanup(&snap, &cfg);
    }

    #[test]
    fn test_account_exists() {
        let (mut mgr, snap, cfg) = test_manager();

        assert!(!mgr.account_exists("drift-user"));
        mgr.add_account("Drift", "drift-user", "pass").unwrap();
        assert!(mgr.account_exists("drift-user"));
        assert!(!mgr.account_exists("other-user"));

        cleanup(&snap, &cfg);
    }

    #[test]
    fn test_list_accounts() {
        let (mut mgr, snap, cfg) = test_manager();

        mgr.add_account("Drift", "drift-user", "p1").unwrap();
        mgr.add_account("Stoleas", "stoleas", "p2").unwrap();
        mgr.add_account("Atervisus", "atervisus", "p3").unwrap();

        let list = mgr.list_accounts();
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].display_name, "Drift");
        assert_eq!(list[1].display_name, "Stoleas");
        assert_eq!(list[2].display_name, "Atervisus");

        cleanup(&snap, &cfg);
    }

    #[tokio::test]
    async fn test_save_and_load_config() {
        let (mut mgr, snap, cfg) = test_manager();

        mgr.add_account("Drift", "drift-user", "pass123").unwrap();
        mgr.add_account("Stoleas", "stoleas", "pass456").unwrap();
        mgr.save().await.unwrap();
        mgr.save_credentials().unwrap();

        // Create a new manager pointing at the same paths
        let cred = CredentialStore::new(&snap);
        let mut mgr2 = AccountManager::new(cred, cfg.clone());
        mgr2.init_credentials("test_master_pass").unwrap();
        mgr2.load().await.unwrap();

        assert_eq!(mgr2.account_count(), 2);
        assert!(mgr2.account_exists("drift-user"));
        assert!(mgr2.account_exists("stoleas"));

        // Passwords should also be loaded from Stronghold
        let pass = mgr2.get_password("drift-user").unwrap();
        assert_eq!(pass, Some("pass123".to_string()));

        cleanup(&snap, &cfg);
    }

    #[tokio::test]
    async fn test_load_nonexistent_config() {
        let snap = temp_snapshot();
        let cfg = temp_config();
        let cred = CredentialStore::new(&snap);
        let mut mgr = AccountManager::new(cred, cfg.clone());
        mgr.init_credentials("pass").unwrap();
        mgr.load().await.unwrap();
        assert!(mgr.accounts.is_empty());

        cleanup(&snap, &cfg);
    }

    #[test]
    fn test_credential_store_accessors() {
        let (mut mgr, snap, cfg) = test_manager();

        // Verify we can access the credential store
        let cred = mgr.credential_store();
        assert!(cred.is_initialized());

        // Mutable access
        let cred_mut = mgr.credential_store_mut();
        assert!(cred_mut.is_initialized());

        cleanup(&snap, &cfg);
    }

    #[test]
    fn test_account_info_serde() {
        let info = AccountInfo {
            display_name: "Drift".to_string(),
            username: "drift-user".to_string(),
        };
        let json = serde_json::to_string(&info).unwrap();
        let decoded: AccountInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(info, decoded);
    }

    #[test]
    fn test_account_info_default() {
        let info = AccountInfo::default();
        assert!(info.display_name.is_empty());
        assert!(info.username.is_empty());
    }

    #[test]
    fn test_multiple_accounts_independent() {
        let (mut mgr, snap, cfg) = test_manager();

        mgr.add_account("A", "user_a", "pass_a").unwrap();
        mgr.add_account("B", "user_b", "pass_b").unwrap();
        mgr.add_account("C", "user_c", "pass_c").unwrap();

        // Each password is independent
        assert_eq!(mgr.get_password("user_a").unwrap().unwrap(), "pass_a");
        assert_eq!(mgr.get_password("user_b").unwrap().unwrap(), "pass_b");
        assert_eq!(mgr.get_password("user_c").unwrap().unwrap(), "pass_c");

        // Remove one doesn't affect others
        mgr.remove_account("user_b").unwrap();
        assert_eq!(mgr.account_count(), 2);
        assert_eq!(mgr.get_password("user_a").unwrap().unwrap(), "pass_a");
        assert_eq!(mgr.get_password("user_c").unwrap().unwrap(), "pass_c");
        assert_eq!(mgr.get_password("user_b").unwrap(), None);

        cleanup(&snap, &cfg);
    }
}