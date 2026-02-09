// Copyright (c) 2026 Elias Bachaalany
// SPDX-License-Identifier: MIT

//! Multi-turn conversation example with session persistence.
//!
//! This example shows how to:
//! - Maintain conversation context across multiple turns
//! - Save and resume sessions for persistence
//! - Continue conversations after program restart

use copilot_sdk::{Client, ResumeSessionConfig, SessionConfig};
use std::fs;
use std::path::Path;

const SESSION_FILE: &str = "session_id.txt";

#[tokio::main]
async fn main() -> copilot_sdk::Result<()> {
    println!("=== Multi-Turn Conversation Example ===\n");

    // Create and start client
    let client = Client::builder().use_stdio(true).build()?;
    client.start().await?;

    // Try to resume existing session, or create new one
    let session = if Path::new(SESSION_FILE).exists() {
        let session_id = fs::read_to_string(SESSION_FILE)?.trim().to_string();
        println!("Resuming session: {}\n", session_id);

        match client
            .resume_session(&session_id, ResumeSessionConfig::default())
            .await
        {
            Ok(s) => s,
            Err(e) => {
                println!("Failed to resume ({}), creating new session...\n", e);
                fs::remove_file(SESSION_FILE).ok();
                let s = client.create_session(SessionConfig::default()).await?;
                fs::write(SESSION_FILE, s.session_id())?;
                println!("New session: {}\n", s.session_id());
                s
            }
        }
    } else {
        let session = client.create_session(SessionConfig::default()).await?;
        fs::write(SESSION_FILE, session.session_id())?;
        println!("Created new session: {}\n", session.session_id());
        session
    };

    // Multi-turn conversation
    let prompts = [
        "My name is Alice and I like Rust programming.",
        "What is my name and what do I like?",
        "Suggest a project I might enjoy based on what you know about me.",
    ];

    for prompt in prompts {
        println!("You: {}\n", prompt);
        let response = session.send_and_collect(prompt, None).await?;
        println!("Assistant: {}\n", response);
        println!("---\n");
    }

    // Show how to clear session
    println!(
        "Session saved to '{}'. Run again to continue the conversation.",
        SESSION_FILE
    );
    println!("Delete the file to start a fresh session.\n");

    client.stop().await;
    Ok(())
}
