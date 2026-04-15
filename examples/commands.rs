// Copyright (c) 2026 Elias Bachaalany
// SPDX-License-Identifier: MIT

//! Commands example demonstrating slash command registration.
//!
//! This example shows how to:
//! - Define and register slash commands
//! - Handle command execution with arguments
//! - Suppress or pass through commands

use copilot_sdk::{
    Client, CommandContext, CommandDefinition, CommandResult, SessionConfig, SessionEventData,
};
use std::io::{self, Write};
use std::sync::Arc;

#[tokio::main]
async fn main() -> copilot_sdk::Result<()> {
    println!("=== Slash Commands Example ===\n");

    let client = Client::builder().use_stdio(true).build()?;
    client.start().await?;

    // Define commands
    let help_cmd = CommandDefinition {
        name: "help".to_string(),
        description: "Show available commands".to_string(),
        handler: Some(Arc::new(|_ctx: &CommandContext| {
            CommandResult {
            message: Some(
                "Available commands:\n  /help - Show this help\n  /status - Show session status\n  /clear - Clear context (suppressed)"
                    .to_string(),
            ),
            suppress: false,
        }
        })),
    };

    let status_cmd = CommandDefinition {
        name: "status".to_string(),
        description: "Show session status".to_string(),
        handler: Some(Arc::new(|ctx: &CommandContext| CommandResult {
            message: Some(format!("Session {} is active.", ctx.session_id)),
            suppress: false,
        })),
    };

    let clear_cmd = CommandDefinition {
        name: "clear".to_string(),
        description: "Clear conversation context".to_string(),
        handler: Some(Arc::new(|_ctx: &CommandContext| {
            println!("[Command] Context cleared locally.");
            CommandResult {
                message: None,
                suppress: true, // Don't send to model
            }
        })),
    };

    // Create session with commands
    let config = SessionConfig {
        commands: Some(vec![help_cmd, status_cmd, clear_cmd]),
        ..Default::default()
    };

    let session = client.create_session(config).await?;
    let mut events = session.subscribe();

    // Demonstrate command execution
    println!("Registered 3 commands: /help, /status, /clear\n");

    // Execute help command directly
    let ctx = CommandContext {
        session_id: session.session_id().to_string(),
        arguments: None,
        raw_input: Some("/help".to_string()),
    };
    let result = session.handle_command_execute("help", &ctx).await?;
    if let Some(msg) = &result.message {
        println!("{}\n", msg);
    }

    // Send a regular message
    println!("Sending a regular message...\n");
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
