// Copyright (c) 2026 Elias Bachaalany
// SPDX-License-Identifier: MIT

//! Basic chat example demonstrating the Copilot SDK.
//!
//! This example shows how to:
//! - Create a client and connect to the Copilot CLI
//! - Create a session
//! - Send a message and receive the response
//! - Handle streaming events

use copilot_sdk::{Client, SessionConfig, SessionEventData};
use std::io::{self, Write};

#[tokio::main]
async fn main() -> copilot_sdk::Result<()> {
    // Create client with default options
    let client = Client::builder().use_stdio(true).build()?;

    // Start the client
    println!("Starting Copilot client...");
    client.start().await?;
    println!("Connected!");

    // Create a session
    let session = client.create_session(SessionConfig::default()).await?;
    println!("Session created: {}", session.session_id());

    // Subscribe to events
    let mut events = session.subscribe();

    // Send a message
    let prompt = "What is the capital of France? Answer in one sentence.";
    println!("\nYou: {}\n", prompt);
    print!("Assistant: ");
    io::stdout().flush().unwrap();

    session.send(prompt).await?;

    // Process events
    loop {
        match events.recv().await {
            Ok(event) => match &event.data {
                // Handle streaming message deltas
                SessionEventData::AssistantMessageDelta(delta) => {
                    print!("{}", delta.delta_content);
                    io::stdout().flush().unwrap();
                }
                // Handle complete messages (if not streaming)
                SessionEventData::AssistantMessage(msg) => {
                    println!("{}", msg.content);
                }
                // Session is idle - we're done
                SessionEventData::SessionIdle(_) => {
                    println!("\n");
                    break;
                }
                // Handle errors
                SessionEventData::SessionError(err) => {
                    eprintln!("\nError: {}", err.message);
                    break;
                }
                // Ignore other events
                _ => {}
            },
            Err(e) => {
                eprintln!("Event error: {:?}", e);
                break;
            }
        }
    }

    // Stop the client
    client.stop().await;
    println!("Done!");

    Ok(())
}
