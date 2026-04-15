// Copyright (c) 2026 Elias Bachaalany
// SPDX-License-Identifier: MIT

//! External server example demonstrating connection to an already-running Copilot server.
//!
//! This example shows how to:
//! - Connect to an external Copilot server via TCP
//! - Configure the client for external server usage
//!
//! Prerequisites:
//!   Start a Copilot CLI server externally, e.g.:
//!     copilot --transport tcp --port 3000

use copilot_sdk::{Client, SessionConfig, SessionEventData};
use std::io::{self, Write};

#[tokio::main]
async fn main() -> copilot_sdk::Result<()> {
    println!("=== External Server Example ===\n");

    let server_url =
        std::env::var("COPILOT_SERVER_URL").unwrap_or_else(|_| "http://localhost:3000".into());

    println!("Connecting to external server at: {}", server_url);
    println!("(Set COPILOT_SERVER_URL to override)\n");

    // Use cli_url to connect to an already-running server via TCP
    let client = Client::builder().cli_url(&server_url).build()?;

    client.start().await?;
    println!("Connected to external server!\n");

    let session = client.create_session(SessionConfig::default()).await?;
    let mut events = session.subscribe();

    session.send("Say hello in one sentence.").await?;

    print!("Assistant: ");
    io::stdout().flush().unwrap();
    while let Ok(event) = events.recv().await {
        match &event.data {
            SessionEventData::AssistantMessageDelta(d) => {
                print!("{}", d.delta_content);
                io::stdout().flush().unwrap();
            }
            SessionEventData::SessionIdle(_) => {
                println!("\n");
                break;
            }
            SessionEventData::SessionError(e) => {
                eprintln!("\nError: {}", e.message);
                break;
            }
            _ => {}
        }
    }

    session.disconnect().await?;
    client.stop().await;
    println!("Done!");
    Ok(())
}
