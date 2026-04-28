// Copyright (c) 2026 Elias Bachaalany
// SPDX-License-Identifier: MIT

//! Blob attachments example demonstrating in-memory data attachments.
//!
//! This example shows how to:
//! - Create blob attachments with inline data (no file path)
//! - Use the `UserMessageAttachment::blob()` constructor
//! - Mix file and blob attachments in a single message

use copilot_sdk::{Client, MessageOptions, SessionConfig, SessionEventData, UserMessageAttachment};
use std::io::{self, Write};

#[tokio::main]
async fn main() -> copilot_sdk::Result<()> {
    println!("=== Blob Attachments Example ===\n");

    let client = Client::builder().use_stdio(true).build()?;
    client.start().await?;

    let session = client.create_session(SessionConfig::default()).await?;
    let mut events = session.subscribe();

    // Create a blob attachment with inline data (no file on disk)
    let csv_data = "name,age,city\nAlice,30,NYC\nBob,25,LA\nCharlie,35,Chicago";
    let blob = UserMessageAttachment::blob(csv_data, "text/csv", "data.csv");

    println!(
        "Sending blob attachment (CSV data, {} bytes)\n",
        csv_data.len()
    );

    let opts = MessageOptions {
        prompt: "Summarize the data in this CSV. Answer in one sentence.".to_string(),
        attachments: Some(vec![blob]),
        mode: None,
        request_headers: None,
    };
    session.send(opts).await?;

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
