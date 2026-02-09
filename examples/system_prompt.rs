// Copyright (c) 2026 Elias Bachaalany
// SPDX-License-Identifier: MIT

//! System prompt example demonstrating custom system message configuration.

use copilot_sdk::{
    Client, SessionConfig, SessionEventData, SystemMessageConfig, SystemMessageMode,
};
use std::io::{self, Write};

#[tokio::main]
async fn main() -> copilot_sdk::Result<()> {
    println!("=== System Prompt Example ===\n");

    let client = Client::builder().use_stdio(true).build()?;
    client.start().await?;

    // Demo: Replace mode - pirate persona
    let config = SessionConfig {
        system_message: Some(SystemMessageConfig {
            mode: Some(SystemMessageMode::Replace),
            content: Some(
                "You are a friendly pirate. Respond in pirate dialect using 'Ahoy!', 'Arr!', etc. Keep responses brief."
                    .to_string(),
            ),
        }),
        ..Default::default()
    };

    let session = client.create_session(config).await?;
    let mut events = session.subscribe();

    println!("Created pirate persona session\n");
    session.send("Hello! Can you help me with coding?").await?;

    print!("Pirate: ");
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
            _ => {}
        }
    }

    // Demo: Append mode
    let append_config = SessionConfig {
        system_message: Some(SystemMessageConfig {
            mode: Some(SystemMessageMode::Append),
            content: Some("Always end responses with a relevant emoji.".to_string()),
        }),
        ..Default::default()
    };

    let session2 = client.create_session(append_config).await?;
    let mut events2 = session2.subscribe();

    println!("Created emoji-ending session\n");
    session2.send("What is 2 + 2?").await?;

    print!("Assistant: ");
    io::stdout().flush().unwrap();
    while let Ok(event) = events2.recv().await {
        match &event.data {
            SessionEventData::AssistantMessageDelta(d) => {
                print!("{}", d.delta_content);
                io::stdout().flush().unwrap();
            }
            SessionEventData::SessionIdle(_) => {
                println!();
                break;
            }
            _ => {}
        }
    }

    client.stop().await;
    Ok(())
}
