// Copyright (c) 2026 Elias Bachaalany
// SPDX-License-Identifier: MIT

//! Streaming example demonstrating real-time event handling.
//!
//! This example shows how to:
//! - Subscribe to session events
//! - Handle streaming message deltas
//! - Track all event types

use copilot_sdk::{Client, SessionConfig, SessionEventData};
use std::io::{self, Write};

#[tokio::main]
async fn main() -> copilot_sdk::Result<()> {
    // Create and start client
    let client = Client::builder().use_stdio(true).build()?;
    println!("Starting Copilot client...");
    client.start().await?;

    // Create session
    let session = client.create_session(SessionConfig::default()).await?;
    println!("Session: {}\n", session.session_id());

    // Subscribe to events
    let mut events = session.subscribe();

    // Send a message that will generate substantial streaming output
    let prompt = "Explain the Rust ownership system in 3 bullet points.";
    println!("You: {}\n", prompt);
    println!("--- Streaming Response ---\n");

    session.send(prompt).await?;

    // Track statistics
    let mut delta_count = 0;
    let mut total_chars = 0;

    // Process events with detailed logging
    loop {
        match events.recv().await {
            Ok(event) => {
                match &event.data {
                    SessionEventData::SessionStart(data) => {
                        println!("[Session started: {}]", data.session_id);
                    }
                    SessionEventData::AssistantTurnStart(_) => {
                        println!("[Turn started]");
                    }
                    SessionEventData::AssistantMessageDelta(delta) => {
                        delta_count += 1;
                        total_chars += delta.delta_content.len();
                        // Print the delta content without newline for streaming effect
                        print!("{}", delta.delta_content);
                        io::stdout().flush().unwrap();
                    }
                    SessionEventData::AssistantMessage(msg) => {
                        // Full message (if not streaming)
                        total_chars += msg.content.len();
                        println!("{}", msg.content);
                    }
                    SessionEventData::AssistantTurnEnd(_) => {
                        println!("\n[Turn ended]");
                    }
                    SessionEventData::AssistantUsage(usage) => {
                        if let (Some(input), Some(output)) =
                            (usage.input_tokens, usage.output_tokens)
                        {
                            println!(
                                "[Usage - Input: {:.0} tokens, Output: {:.0} tokens]",
                                input, output
                            );
                        }
                    }
                    SessionEventData::SessionIdle(_) => {
                        println!("[Session idle]");
                        break;
                    }
                    SessionEventData::SessionError(err) => {
                        eprintln!("\n[Error: {}]", err.message);
                        break;
                    }
                    // Log other events
                    other => {
                        println!("[Event: {:?}]", std::mem::discriminant(other));
                    }
                }
            }
            Err(e) => {
                eprintln!("Event error: {:?}", e);
                break;
            }
        }
    }

    // Print statistics
    println!("\n--- Statistics ---");
    println!("Delta events received: {}", delta_count);
    println!("Total characters: {}", total_chars);

    client.stop().await;
    println!("\nDone!");
    Ok(())
}
