// Copyright (c) 2026 Elias Bachaalany
// SPDX-License-Identifier: MIT

//! Reasoning effort example demonstrating reasoning effort configuration.
//!
//! This example shows how to:
//! 1. List available models and check for reasoning effort support
//! 2. Create a session with a specific reasoning effort level
//! 3. Monitor usage data (input/output tokens, cost)

use copilot_sdk::{Client, SessionConfig, SessionEventData};
use std::io::Write;
use std::sync::Arc;

#[tokio::main]
async fn main() -> copilot_sdk::Result<()> {
    let client = Client::builder().build()?;

    println!("=== Reasoning Effort Example ===\n");
    println!("Reasoning effort controls how much 'thinking' a model does.");
    println!("Values: low, medium, high, xhigh\n");

    client.start().await?;

    // List models and show which support reasoning effort
    println!("--- Available Models ---");
    let models = client.list_models().await?;

    for model in &models {
        print!("  {}", model.id);

        if model.capabilities.supports.reasoning_effort {
            print!(" [reasoning effort: YES]");
        }

        if let Some(ref efforts) = model.supported_reasoning_efforts {
            print!(" (levels: {})", efforts.join(", "));
        }

        if let Some(ref default) = model.default_reasoning_effort {
            print!(" [default: {default}]");
        }

        println!();
    }

    println!();

    // Create session with reasoning effort
    let config = SessionConfig {
        reasoning_effort: Some("medium".into()),
        ..Default::default()
    };

    let session = client.create_session(config).await?;
    println!("Session created with reasoning_effort = 'medium'");
    println!("Session ID: {}\n", session.session_id());

    let idle_notify = Arc::new(tokio::sync::Notify::new());
    let idle_clone = Arc::clone(&idle_notify);

    let _unsub = session
        .on(move |event| match &event.data {
            SessionEventData::AssistantMessage(msg) => {
                println!("\nAssistant: {}", msg.content);
            }
            SessionEventData::AssistantUsage(usage) => {
                print!("[Usage:");
                if let Some(input) = usage.input_tokens {
                    print!(" {input} in");
                }
                if let Some(output) = usage.output_tokens {
                    print!(" / {output} out");
                }
                if let Some(cost) = usage.cost {
                    print!(" / cost={cost}");
                }
                println!("]");
            }
            SessionEventData::SessionIdle(_) => {
                idle_clone.notify_one();
            }
            _ => {}
        })
        .await;

    println!("Chat with reasoning effort enabled. Type 'quit' to exit.\n");
    print!("> ");
    std::io::stdout().flush().unwrap();

    let stdin = std::io::stdin();
    let mut line = String::new();

    loop {
        line.clear();
        if stdin.read_line(&mut line).unwrap() == 0 {
            break;
        }
        let input = line.trim();

        if input == "quit" || input == "exit" {
            break;
        }
        if input.is_empty() {
            print!("> ");
            std::io::stdout().flush().unwrap();
            continue;
        }

        session.send(input).await?;
        idle_notify.notified().await;
        print!("\n> ");
        std::io::stdout().flush().unwrap();
    }

    session.destroy().await?;
    client.stop().await;

    Ok(())
}
