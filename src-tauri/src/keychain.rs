//! macOS Keychain integration for secure credential storage.
//!
//! Convex auth tokens and (for admin machines) R2 credentials are stored in
//! the system Keychain rather than in plaintext config files. This module
//! wraps the `security-framework` crate with SessionSync-specific helpers.

use security_framework::passwords::{
    delete_generic_password, get_generic_password, set_generic_password,
};

use crate::error::{SyncError, SyncResult};

/// The Keychain service name used for all SessionSync entries.
pub const SERVICE_NAME: &str = "com.sessionsync.app";

// ── Keychain account keys ───────────────────────────────────────────────
// Each credential type gets a distinct "account" within the service.

const ACCOUNT_CONVEX_TOKEN: &str = "convex-auth-token";
const ACCOUNT_R2_ACCESS_KEY_ID: &str = "r2-access-key-id";
const ACCOUNT_R2_SECRET_ACCESS_KEY: &str = "r2-secret-access-key";
const ACCOUNT_R2_ACCOUNT_ID: &str = "r2-account-id";
const ACCOUNT_R2_BUCKET_NAME: &str = "r2-bucket-name";

// ── Generic low-level operations ────────────────────────────────────────

/// Store a credential in the macOS Keychain.
///
/// If an entry already exists for the given `(service, account)` pair it is
/// overwritten (delete + re-create) to avoid `errSecDuplicateItem`.
pub fn store_credential(service: &str, account: &str, password: &str) -> SyncResult<()> {
    // Attempt to delete any pre-existing entry first (ignore errors — it
    // may not exist yet).
    let _ = delete_generic_password(service, account);

    set_generic_password(service, account, password.as_bytes()).map_err(|e| {
        SyncError::KeychainError {
            message: format!("failed to store credential for account '{account}': {e}"),
        }
    })
}

/// Retrieve a credential from the macOS Keychain.
///
/// Returns `None` if no entry exists for the given `(service, account)` pair.
pub fn retrieve_credential(service: &str, account: &str) -> SyncResult<Option<String>> {
    match get_generic_password(service, account) {
        Ok(bytes) => {
            let value = String::from_utf8(bytes.to_vec()).map_err(|_| SyncError::KeychainError {
                message: format!(
                    "credential for account '{account}' contains invalid UTF-8"
                ),
            })?;
            Ok(Some(value))
        }
        Err(e) => {
            // `errSecItemNotFound` (-25300) means the credential simply
            // does not exist. Any other error is unexpected.
            let code = e.code();
            if code == -25300 {
                Ok(None)
            } else {
                Err(SyncError::KeychainError {
                    message: format!(
                        "failed to retrieve credential for account '{account}': {e}"
                    ),
                })
            }
        }
    }
}

/// Delete a credential from the macOS Keychain.
///
/// Silently succeeds if no entry exists.
pub fn delete_credential(service: &str, account: &str) -> SyncResult<()> {
    match delete_generic_password(service, account) {
        Ok(()) => Ok(()),
        Err(e) => {
            let code = e.code();
            if code == -25300 {
                // Not found — nothing to delete.
                Ok(())
            } else {
                Err(SyncError::KeychainError {
                    message: format!(
                        "failed to delete credential for account '{account}': {e}"
                    ),
                })
            }
        }
    }
}

// ── Convex auth token ───────────────────────────────────────────────────

/// Store the Convex authentication token in the Keychain.
pub fn store_convex_token(token: &str) -> SyncResult<()> {
    store_credential(SERVICE_NAME, ACCOUNT_CONVEX_TOKEN, token)
}

/// Retrieve the Convex authentication token, or `None` if not yet stored.
pub fn get_convex_token() -> SyncResult<Option<String>> {
    retrieve_credential(SERVICE_NAME, ACCOUNT_CONVEX_TOKEN)
}

/// Delete the stored Convex authentication token.
pub fn delete_convex_token() -> SyncResult<()> {
    delete_credential(SERVICE_NAME, ACCOUNT_CONVEX_TOKEN)
}

// ── R2 credentials (admin machines only) ────────────────────────────────

/// All four values needed to interact with Cloudflare R2.
#[derive(Debug, Clone)]
pub struct R2Credentials {
    pub account_id: String,
    pub access_key_id: String,
    pub secret_access_key: String,
    pub bucket_name: String,
}

/// Store R2 credentials in the Keychain (one entry per field).
pub fn store_r2_credentials(creds: &R2Credentials) -> SyncResult<()> {
    store_credential(SERVICE_NAME, ACCOUNT_R2_ACCOUNT_ID, &creds.account_id)?;
    store_credential(SERVICE_NAME, ACCOUNT_R2_ACCESS_KEY_ID, &creds.access_key_id)?;
    store_credential(
        SERVICE_NAME,
        ACCOUNT_R2_SECRET_ACCESS_KEY,
        &creds.secret_access_key,
    )?;
    store_credential(SERVICE_NAME, ACCOUNT_R2_BUCKET_NAME, &creds.bucket_name)?;
    Ok(())
}

/// Retrieve R2 credentials from the Keychain.
///
/// Returns `None` if any of the four fields are missing — the credential
/// set must be complete.
pub fn get_r2_credentials() -> SyncResult<Option<R2Credentials>> {
    let account_id = retrieve_credential(SERVICE_NAME, ACCOUNT_R2_ACCOUNT_ID)?;
    let access_key_id = retrieve_credential(SERVICE_NAME, ACCOUNT_R2_ACCESS_KEY_ID)?;
    let secret_access_key = retrieve_credential(SERVICE_NAME, ACCOUNT_R2_SECRET_ACCESS_KEY)?;
    let bucket_name = retrieve_credential(SERVICE_NAME, ACCOUNT_R2_BUCKET_NAME)?;

    match (account_id, access_key_id, secret_access_key, bucket_name) {
        (Some(aid), Some(aki), Some(sak), Some(bn)) => Ok(Some(R2Credentials {
            account_id: aid,
            access_key_id: aki,
            secret_access_key: sak,
            bucket_name: bn,
        })),
        _ => Ok(None),
    }
}

/// Delete all stored R2 credentials from the Keychain.
pub fn delete_r2_credentials() -> SyncResult<()> {
    delete_credential(SERVICE_NAME, ACCOUNT_R2_ACCOUNT_ID)?;
    delete_credential(SERVICE_NAME, ACCOUNT_R2_ACCESS_KEY_ID)?;
    delete_credential(SERVICE_NAME, ACCOUNT_R2_SECRET_ACCESS_KEY)?;
    delete_credential(SERVICE_NAME, ACCOUNT_R2_BUCKET_NAME)?;
    Ok(())
}
