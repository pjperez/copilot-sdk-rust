// Copyright (c) 2026 Elias Bachaalany
// SPDX-License-Identifier: MIT

//! Elicitation example demonstrating interactive UI dialogs.
//!
//! This example shows how to:
//! - Register an elicitation handler for user prompts
//! - Handle different elicitation types (confirm, select, input)
//! - Return structured responses from the handler

use copilot_sdk::{
    Client, ElicitationContext, ElicitationHandler, ElicitationResult, SessionConfig,
    SessionEventData,
};
use std::io::{self, Write};
use std::sync::Arc;

#[tokio::main]
async fn main() -> copilot_sdk::Result<()> {
    println!("=== Elicitation Example ===\n");

    let client = Client::builder().use_stdio(true).build()?;
    client.start().await?;

    // Create session with elicitation enabled
    let config = SessionConfig {
        request_elicitation: Some(true),
        ..Default::default()
    };

    let session = client.create_session(config).await?;

    // Register elicitation handler on the session
    let handler: ElicitationHandler = Arc::new(|ctx: &ElicitationContext| {
        println!(
            "\n[Elicitation] Type: {}, Message: {}",
            ctx.params.elicitation_type, ctx.params.message
        );

        match ctx.params.elicitation_type.as_str() {
            "confirm" => {
                println!("[Elicitation] Auto-confirming");
                ElicitationResult::accept(serde_json::json!(true))
            }
            "select" => {
                if let Some(options) = &ctx.params.options {
                    if let Some(first) = options.first() {
                        println!("[Elicitation] Auto-selecting: {}", first.label);
                        return ElicitationResult::accept(serde_json::json!(first.value));
                    }
                }
                ElicitationResult::dismiss()
            }
            "input" => {
                println!("[Elicitation] Auto-providing input");
                ElicitationResult::accept(serde_json::json!("auto-response"))
            }
            _ => {
                println!("[Elicitation] Unknown type, dismissing");
                ElicitationResult::dismiss()
            }
        }
    });
    session.register_elicitation_handler(handler).await;
    let mut events = session.subscribe();

    // Verify handler is registered
    assert!(session.has_elicitation_handler().await);
    println!("Elicitation handler registered.\n");

    // Send a message that might trigger elicitation
    session
        .send("What is the capital of France? Answer briefly.")
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
