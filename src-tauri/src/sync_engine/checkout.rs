//! Checkout coordination via Convex HTTP API.
//!
//! Since there is no official Convex Rust SDK, all communication happens
//! through direct HTTP calls to the Convex deployment's REST endpoints:
//!
//! - **Mutations**: `POST /api/mutation`  (checkout, release, heartbeat, etc.)
//! - **Actions**:   `POST /api/action`    (presigned URL generation)
//! - **Queries**:   `POST /api/query`     (session state, version list)
//!
//! The `ConvexClient` handles authentication (Bearer token), serialization,
//! and error mapping.  The `CheckoutManager` provides the high-level
//! checkout/release/claim/request/heartbeat operations.

use std::sync::Arc;

use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use serde_json::{json, Value};
use tracing::{debug, info, warn};

use crate::error::{SyncError, SyncResult};

// ── ConvexClient ────────────────────────────────────────────────────────

/// Low-level HTTP client for the Convex deployment API.
///
/// Wraps `reqwest::Client` with Convex-specific URL construction,
/// authentication, and error mapping.
#[derive(Clone)]
pub struct ConvexClient {
    /// Base URL of the Convex deployment (e.g. "https://foo-bar-123.convex.cloud").
    base_url: String,
    /// Auth token for the Convex deployment.
    auth_token: String,
    /// Shared HTTP client (connection pooling).
    http: reqwest::Client,
}

impl ConvexClient {
    /// Create a new Convex client.
    ///
    /// `base_url` should be the deployment URL without a trailing slash.
    /// `auth_token` is the Convex auth token (stored in macOS Keychain).
    pub fn new(base_url: &str, auth_token: &str) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("Failed to build HTTP client");

        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            auth_token: auth_token.to_string(),
            http,
        }
    }

    /// Build the standard headers for Convex API calls.
    fn headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        if !self.auth_token.is_empty() {
            if let Ok(val) = HeaderValue::from_str(&format!("Bearer {}", self.auth_token)) {
                headers.insert(AUTHORIZATION, val);
            }
        }
        headers
    }

    /// Call a Convex mutation.
    ///
    /// Mutations are state-changing operations (checkout, release, heartbeat,
    /// create version).  They are called via `POST /api/mutation`.
    pub async fn mutation(
        &self,
        function_name: &str,
        args: Value,
    ) -> SyncResult<Value> {
        let url = format!("{}/api/mutation", self.base_url);
        let body = json!({
            "path": function_name,
            "args": args,
            "format": "json",
        });

        debug!(function = function_name, "Calling Convex mutation");

        let response = self
            .http
            .post(&url)
            .headers(self.headers())
            .json(&body)
            .send()
            .await
            .map_err(SyncError::Http)?;

        self.parse_response(response, function_name).await
    }

    /// Call a Convex action.
    ///
    /// Actions can have side effects and call external services (e.g. generate
    /// presigned URLs by calling the R2 API with server-side credentials).
    /// They are called via `POST /api/action`.
    pub async fn action(
        &self,
        function_name: &str,
        args: Value,
    ) -> SyncResult<Value> {
        let url = format!("{}/api/action", self.base_url);
        let body = json!({
            "path": function_name,
            "args": args,
            "format": "json",
        });

        debug!(function = function_name, "Calling Convex action");

        let response = self
            .http
            .post(&url)
            .headers(self.headers())
            .json(&body)
            .send()
            .await
            .map_err(SyncError::Http)?;

        self.parse_response(response, function_name).await
    }

    /// Call a Convex query (read-only).
    ///
    /// Queries are one-shot reads of the Convex database.  They do not
    /// subscribe to real-time updates (that's handled by the React frontend
    /// via the official JS client).
    pub async fn query(
        &self,
        function_name: &str,
        args: Value,
    ) -> SyncResult<Value> {
        let url = format!("{}/api/query", self.base_url);
        let body = json!({
            "path": function_name,
            "args": args,
            "format": "json",
        });

        debug!(function = function_name, "Calling Convex query");

        let response = self
            .http
            .post(&url)
            .headers(self.headers())
            .json(&body)
            .send()
            .await
            .map_err(SyncError::Http)?;

        self.parse_response(response, function_name).await
    }

    /// Parse a Convex HTTP API response.
    ///
    /// Convex returns `{"status": "success", "value": ...}` on success and
    /// `{"status": "error", "errorMessage": "..."}` on failure.
    async fn parse_response(
        &self,
        response: reqwest::Response,
        function_name: &str,
    ) -> SyncResult<Value> {
        let status_code = response.status();

        if !status_code.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(SyncError::ConvexApi {
                function: function_name.to_string(),
                message: format!("HTTP {}: {}", status_code, body),
            });
        }

        let body: Value = response.json().await.map_err(SyncError::Http)?;

        // Check Convex-level status field.
        let convex_status = body
            .get("status")
            .and_then(|s| s.as_str())
            .unwrap_or("unknown");

        if convex_status == "error" {
            let error_message = body
                .get("errorMessage")
                .and_then(|m| m.as_str())
                .unwrap_or("Unknown Convex error")
                .to_string();

            return Err(SyncError::ConvexApi {
                function: function_name.to_string(),
                message: error_message,
            });
        }

        Ok(body)
    }
}

impl std::fmt::Debug for ConvexClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConvexClient")
            .field("base_url", &self.base_url)
            .field("auth_token", &"[redacted]")
            .finish()
    }
}

// ── CheckoutManager ─────────────────────────────────────────────────────

/// High-level checkout coordination.
///
/// All checkout-related operations go through Convex mutations which perform
/// server-side validation (e.g. atomic compare-and-swap for checkout, caller
/// verification for release).
pub struct CheckoutManager {
    convex: Arc<ConvexClient>,
}

impl CheckoutManager {
    pub fn new(convex: Arc<ConvexClient>) -> Self {
        Self { convex }
    }

    /// Attempt to check out a session.
    ///
    /// Calls `sessions:checkoutSession` which performs an atomic
    /// compare-and-swap: sets `checkedOutBy = machine_id` only if the field
    /// is currently null (or the session status is "stale").
    ///
    /// Returns `Ok(())` on success, or `SyncError::CheckoutConflict` if
    /// another machine already holds the checkout.
    pub async fn checkout(
        &self,
        session_id: &str,
        machine_id: &str,
    ) -> SyncResult<()> {
        info!(
            session_id = session_id,
            machine_id = machine_id,
            "Requesting checkout"
        );

        let result = self
            .convex
            .mutation(
                "sessions:checkoutSession",
                json!({
                    "sessionId": session_id,
                    "machineId": machine_id,
                }),
            )
            .await;

        match result {
            Ok(_) => {
                info!(
                    session_id = session_id,
                    machine_id = machine_id,
                    "Checkout acquired"
                );
                Ok(())
            }
            Err(SyncError::ConvexApi { ref message, .. })
                if message.contains("already checked out") =>
            {
                // Extract the holder from the error message if possible.
                Err(SyncError::CheckoutConflict {
                    session_id: session_id.to_string(),
                    held_by: extract_holder(message),
                })
            }
            Err(e) => Err(e),
        }
    }

    /// Release a checkout, optionally attaching a release note.
    ///
    /// Calls `sessions:releaseSession` which validates that `checkedOutBy`
    /// matches the requesting machine before clearing the lock.
    pub async fn release(
        &self,
        session_id: &str,
        machine_id: &str,
        note: Option<&str>,
    ) -> SyncResult<()> {
        info!(
            session_id = session_id,
            machine_id = machine_id,
            has_note = note.is_some(),
            "Releasing checkout"
        );

        self.convex
            .mutation(
                "sessions:releaseSession",
                json!({
                    "sessionId": session_id,
                    "machineId": machine_id,
                    "releaseNote": note,
                }),
            )
            .await?;

        info!(
            session_id = session_id,
            "Checkout released"
        );
        Ok(())
    }

    /// Claim a stale session.
    ///
    /// Only allowed when the session's heartbeat has expired (status = "stale").
    /// The previous checkout holder's auto-pushed state is preserved.
    pub async fn claim(
        &self,
        session_id: &str,
        machine_id: &str,
    ) -> SyncResult<()> {
        info!(
            session_id = session_id,
            machine_id = machine_id,
            "Claiming stale session"
        );

        self.convex
            .mutation(
                "sessions:claimSession",
                json!({
                    "sessionId": session_id,
                    "machineId": machine_id,
                }),
            )
            .await?;

        info!(
            session_id = session_id,
            "Session claimed"
        );
        Ok(())
    }

    /// Request that the current checkout holder release the session.
    ///
    /// This sends a notification to the holder (via Convex subscription ->
    /// Tauri event -> macOS notification on their machine). It does NOT
    /// force-release — the request is advisory only.
    pub async fn request(
        &self,
        session_id: &str,
        machine_id: &str,
    ) -> SyncResult<()> {
        info!(
            session_id = session_id,
            machine_id = machine_id,
            "Requesting session release"
        );

        self.convex
            .mutation(
                "sessions:requestSession",
                json!({
                    "sessionId": session_id,
                    "requestedBy": machine_id,
                }),
            )
            .await?;

        info!(
            session_id = session_id,
            "Release request sent"
        );
        Ok(())
    }

    /// Send a heartbeat to Convex.
    ///
    /// While holding a checkout, the engine sends a heartbeat every 5 minutes
    /// to prove the machine is still alive. If heartbeats are missed for 30
    /// minutes, Convex marks the session as "stale" and other engineers can
    /// claim it.
    pub async fn send_heartbeat(
        &self,
        session_id: &str,
        machine_id: &str,
    ) -> SyncResult<()> {
        debug!(
            session_id = session_id,
            machine_id = machine_id,
            "Sending heartbeat"
        );

        self.convex
            .mutation(
                "machines:heartbeat",
                json!({
                    "sessionId": session_id,
                    "machineId": machine_id,
                }),
            )
            .await?;

        debug!(
            session_id = session_id,
            "Heartbeat sent"
        );
        Ok(())
    }

    /// Query the current session state from Convex.
    ///
    /// Used during WAL replay and periodic state reconciliation.
    pub async fn get_session_state(
        &self,
        session_id: &str,
    ) -> SyncResult<Value> {
        self.convex
            .query(
                "sessions:get",
                json!({ "sessionId": session_id }),
            )
            .await
    }

    /// Get the latest version number for a session.
    pub async fn get_latest_version(
        &self,
        session_id: &str,
    ) -> SyncResult<Option<u64>> {
        let response = self
            .convex
            .query(
                "versions:getLatest",
                json!({ "sessionId": session_id }),
            )
            .await?;

        let version = response
            .get("value")
            .and_then(|v| v.get("versionNumber"))
            .and_then(|v| v.as_u64());

        Ok(version)
    }

    /// Request a batch of presigned upload URLs from Convex.
    ///
    /// Returns a vec of `(object_key, presigned_url)` pairs.
    pub async fn request_upload_urls(
        &self,
        session_id: &str,
        machine_id: &str,
        object_keys: &[String],
    ) -> SyncResult<Vec<(String, String)>> {
        info!(
            session_id = session_id,
            count = object_keys.len(),
            "Requesting presigned upload URLs"
        );

        let response = self
            .convex
            .action(
                "presignedUrls:requestBatchUploadUrls",
                json!({
                    "sessionId": session_id,
                    "machineId": machine_id,
                    "objectKeys": object_keys,
                }),
            )
            .await?;

        let urls = response
            .get("value")
            .and_then(|v| v.as_array())
            .ok_or_else(|| SyncError::ConvexApi {
                function: "presignedUrls:requestBatchUploadUrls".to_string(),
                message: "Expected array of URLs in response".to_string(),
            })?;

        let mut result = Vec::with_capacity(urls.len());
        for item in urls {
            let key = item
                .get("key")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let url = item
                .get("url")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            result.push((key, url));
        }

        Ok(result)
    }

    /// Request presigned download URLs for a list of object keys.
    pub async fn request_download_urls(
        &self,
        session_id: &str,
        machine_id: &str,
        object_keys: &[String],
    ) -> SyncResult<Vec<(String, String)>> {
        info!(
            session_id = session_id,
            count = object_keys.len(),
            "Requesting presigned download URLs"
        );

        let response = self
            .convex
            .action(
                "presignedUrls:requestBatchDownloadUrls",
                json!({
                    "sessionId": session_id,
                    "machineId": machine_id,
                    "objectKeys": object_keys,
                }),
            )
            .await?;

        let urls = response
            .get("value")
            .and_then(|v| v.as_array())
            .ok_or_else(|| SyncError::ConvexApi {
                function: "presignedUrls:requestBatchDownloadUrls".to_string(),
                message: "Expected array of URLs in response".to_string(),
            })?;

        let mut result = Vec::with_capacity(urls.len());
        for item in urls {
            let key = item
                .get("key")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let url = item
                .get("url")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            result.push((key, url));
        }

        Ok(result)
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────

/// Try to extract the holder machine ID from a Convex error message.
fn extract_holder(message: &str) -> String {
    // Convex might return something like "Session already checked out by 'austin-macbook'"
    // Try to extract the holder name.
    if let Some(start) = message.find('\'') {
        if let Some(end) = message[start + 1..].find('\'') {
            return message[start + 1..start + 1 + end].to_string();
        }
    }
    "another machine".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_holder_with_quotes() {
        let msg = "Session already checked out by 'austin-macbook'";
        assert_eq!(extract_holder(msg), "austin-macbook");
    }

    #[test]
    fn test_extract_holder_no_quotes() {
        let msg = "Session already checked out";
        assert_eq!(extract_holder(msg), "another machine");
    }

    #[test]
    fn test_convex_client_debug_redacts_token() {
        let client = ConvexClient::new("https://example.convex.cloud", "secret-token");
        let debug_str = format!("{:?}", client);
        assert!(!debug_str.contains("secret-token"));
        assert!(debug_str.contains("[redacted]"));
    }
}
