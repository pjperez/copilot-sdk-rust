// Copyright (c) 2026 Elias Bachaalany
// SPDX-License-Identifier: MIT

//! Set model example demonstrating runtime model switching.
//!
//! This example shows how to:
//! - Switch the model used by a session at runtime
//! - Use `session.set_model()` to change models mid-conversation

use copilot_sdk::{Client, SessionConfig, SessionEventData};
use std::io::{self, Write};

#[tokio::main]
async fn main() -> copilot_sdk::Result<()> {
    println!("=== Set Model Example ===\n");

    let client = Client::builder().use_stdio(true).build()?;
    client.start().await?;

    // List available models
    let models = client.list_models().await?;
    println!("Available models:");
    for model in &models {
        println!("  - {} ({})", model.name, model.id);
    }
    println!();

    let session = client.create_session(SessionConfig::default()).await?;
    let mut events = session.subscribe();

    // Switch model at runtime (if multiple are available)
    if models.len() > 1 {
        let new_model = &models[1].id;
        println!("Switching to model: {}\n", new_model);
        session.set_model(new_model, None, None).await?;
    }

    session
        .send("What model are you? Answer in one sentence.")
        .await?;

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
