// Copyright (c) 2026 Elias Bachaalany
// SPDX-License-Identifier: MIT

//! Demonstrates plugging a custom `SessionFsProvider` so the runtime calls
//! back into the SDK for session-state filesystem operations.
//!
//! This example uses an in-memory map. Real hosts can back the provider
//! with object storage, sandboxed FS, etc.

use async_trait::async_trait;
use copilot_sdk::{
    Client, CreateSessionFsHandler, Result, SessionConfig, SessionFsError, SessionFsFileInfo,
    SessionFsProvider, SessionFsProviderEntry, SessionFsSetProviderRequest, SharedSessionFsProvider,
};
use copilot_sdk::types::SessionFsConventions;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

struct InMemoryFs {
    files: Mutex<HashMap<String, String>>,
}

impl InMemoryFs {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            files: Mutex::new(HashMap::new()),
        })
    }
}

#[async_trait]
impl SessionFsProvider for InMemoryFs {
    async fn read_file(&self, path: &str) -> std::result::Result<String, SessionFsError> {
        let files = self.files.lock().await;
        files
            .get(path)
            .cloned()
            .ok_or_else(|| SessionFsError::enoent(format!("not found: {path}")))
    }

    async fn write_file(
        &self,
        path: &str,
        content: &str,
        _mode: Option<u32>,
    ) -> std::result::Result<(), SessionFsError> {
        self.files
            .lock()
            .await
            .insert(path.to_string(), content.to_string());
        Ok(())
    }

    async fn append_file(
        &self,
        path: &str,
        content: &str,
        _mode: Option<u32>,
    ) -> std::result::Result<(), SessionFsError> {
        let mut files = self.files.lock().await;
        files.entry(path.to_string()).or_default().push_str(content);
        Ok(())
    }

    async fn exists(&self, path: &str) -> std::result::Result<bool, SessionFsError> {
        Ok(self.files.lock().await.contains_key(path))
    }

    async fn stat(&self, path: &str) -> std::result::Result<SessionFsFileInfo, SessionFsError> {
        let files = self.files.lock().await;
        let content = files
            .get(path)
            .ok_or_else(|| SessionFsError::enoent(format!("not found: {path}")))?;
        Ok(SessionFsFileInfo {
            is_file: true,
            is_directory: false,
            size: content.len() as u64,
            mtime: chrono::Utc::now().to_rfc3339(),
            birthtime: chrono::Utc::now().to_rfc3339(),
        })
    }

    async fn mkdir(
        &self,
        _path: &str,
        _recursive: bool,
        _mode: Option<u32>,
    ) -> std::result::Result<(), SessionFsError> {
        Ok(())
    }

    async fn readdir(&self, _path: &str) -> std::result::Result<Vec<String>, SessionFsError> {
        Ok(self.files.lock().await.keys().cloned().collect())
    }

    async fn readdir_with_types(
        &self,
        _path: &str,
    ) -> std::result::Result<Vec<SessionFsProviderEntry>, SessionFsError> {
        Ok(self
            .files
            .lock()
            .await
            .keys()
            .map(|name| SessionFsProviderEntry {
                name: name.clone(),
                is_file: true,
                is_directory: false,
            })
            .collect())
    }

    async fn rm(
        &self,
        path: &str,
        _recursive: bool,
        _force: bool,
    ) -> std::result::Result<(), SessionFsError> {
        self.files.lock().await.remove(path);
        Ok(())
    }

    async fn rename(&self, src: &str, dest: &str) -> std::result::Result<(), SessionFsError> {
        let mut files = self.files.lock().await;
        if let Some(content) = files.remove(src) {
            files.insert(dest.to_string(), content);
        }
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // The runtime must be told the SDK is the FS provider — without
    // `sessionFs.setProvider`, inbound `sessionFs.*` callbacks are never sent.
    let client = Client::builder()
        .session_fs(SessionFsSetProviderRequest {
            initial_cwd: std::env::current_dir()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|_| ".".into()),
            session_state_path: ".copilot-sessions".into(),
            #[cfg(target_os = "windows")]
            conventions: SessionFsConventions::Windows,
            #[cfg(not(target_os = "windows"))]
            conventions: SessionFsConventions::Posix,
        })
        .build()?;
    client.start().await?;

    let factory =
        CreateSessionFsHandler::new(|_session_id| -> SharedSessionFsProvider { InMemoryFs::new() });

    let session = client
        .create_session(SessionConfig {
            create_session_fs_handler: Some(factory),
            ..Default::default()
        })
        .await?;

    session
        .send("Write a small markdown summary to plan.md, then read it back.")
        .await?;

    session.disconnect().await?;
    client.stop().await;
    Ok(())
}
