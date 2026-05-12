// Copyright (c) 2026 Elias Bachaalany
// SPDX-License-Identifier: MIT

//! Server-side session filesystem provider.
//!
//! The Copilot CLI runtime can call back into the SDK to read/write the
//! session-state filesystem (used for resumable workspace files,
//! infinite-session checkpoints, etc.). This module defines the
//! [`SessionFsProvider`] trait that hosts can implement to plug a custom
//! storage backend (in-memory, object storage, sandboxed FS, …).
//!
//! Mirrors Python's `copilot.session_fs_provider`. The SDK ships no default
//! provider; if you don't implement one, the runtime falls back to its own
//! internal storage.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Error code reported back to the runtime for failed FS operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionFsErrorCode {
    /// File or directory does not exist.
    #[serde(rename = "ENOENT")]
    NoEnt,
    /// Any other failure (unsupported by the provider, IO error, etc.).
    #[serde(rename = "UNKNOWN")]
    Unknown,
}

/// Wire-format error envelope returned by FS provider methods.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionFsError {
    pub code: SessionFsErrorCode,
    pub message: String,
}

impl SessionFsError {
    pub fn enoent(message: impl Into<String>) -> Self {
        Self {
            code: SessionFsErrorCode::NoEnt,
            message: message.into(),
        }
    }

    pub fn unknown(message: impl Into<String>) -> Self {
        Self {
            code: SessionFsErrorCode::Unknown,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for SessionFsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}

impl std::error::Error for SessionFsError {}

/// Convert any `std::io::Error` into a [`SessionFsError`], mapping
/// `NotFound` to `ENOENT` and everything else to `UNKNOWN`.
impl From<std::io::Error> for SessionFsError {
    fn from(err: std::io::Error) -> Self {
        if err.kind() == std::io::ErrorKind::NotFound {
            SessionFsError::enoent(err.to_string())
        } else {
            SessionFsError::unknown(err.to_string())
        }
    }
}

/// Metadata returned by [`SessionFsProvider::stat`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionFsFileInfo {
    pub is_file: bool,
    pub is_directory: bool,
    pub size: u64,
    /// Modification time in ISO 8601 format.
    pub mtime: String,
    /// Birth (creation) time in ISO 8601 format.
    pub birthtime: String,
}

/// Single entry returned by [`SessionFsProvider::readdir_with_types`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionFsProviderEntry {
    pub name: String,
    pub is_file: bool,
    pub is_directory: bool,
}

/// Trait implemented by hosts to provide a custom session-FS backend.
///
/// Each method takes the absolute `path` (or `src`/`dest`) handed in by the
/// runtime and returns a `Result` whose error variant is sent back as a
/// [`SessionFsError`]. Use the conversions from `std::io::Error` for the
/// common case of bridging to a `tokio::fs`-backed implementation.
///
/// Implementations are stored in an [`Arc`] so they can be shared across
/// concurrent inbound RPC requests.
#[async_trait]
pub trait SessionFsProvider: Send + Sync {
    /// Read the full content of a file. Return `SessionFsError::enoent` if
    /// the file doesn't exist.
    async fn read_file(&self, path: &str) -> Result<String, SessionFsError>;

    /// Write `content` to `path`, creating parent directories as needed.
    async fn write_file(
        &self,
        path: &str,
        content: &str,
        mode: Option<u32>,
    ) -> Result<(), SessionFsError>;

    /// Append `content` to `path`, creating parent directories as needed.
    async fn append_file(
        &self,
        path: &str,
        content: &str,
        mode: Option<u32>,
    ) -> Result<(), SessionFsError>;

    /// Whether the given path exists.
    async fn exists(&self, path: &str) -> Result<bool, SessionFsError>;

    /// Return metadata for the given path.
    async fn stat(&self, path: &str) -> Result<SessionFsFileInfo, SessionFsError>;

    /// Create a directory; if `recursive` is `true`, create parents too.
    async fn mkdir(
        &self,
        path: &str,
        recursive: bool,
        mode: Option<u32>,
    ) -> Result<(), SessionFsError>;

    /// List the names of entries in `path`.
    async fn readdir(&self, path: &str) -> Result<Vec<String>, SessionFsError>;

    /// List entries in `path` along with their file/directory flags.
    async fn readdir_with_types(
        &self,
        path: &str,
    ) -> Result<Vec<SessionFsProviderEntry>, SessionFsError>;

    /// Remove a file or directory.
    async fn rm(&self, path: &str, recursive: bool, force: bool) -> Result<(), SessionFsError>;

    /// Move/rename a file or directory.
    async fn rename(&self, src: &str, dest: &str) -> Result<(), SessionFsError>;
}

/// Convenience alias for a shareable [`SessionFsProvider`] trait object.
pub type SharedSessionFsProvider = Arc<dyn SessionFsProvider>;
