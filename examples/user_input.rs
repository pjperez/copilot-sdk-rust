// Copyright (c) 2026 Elias Bachaalany
// SPDX-License-Identifier: MIT

//! Example demonstrating the user input handler for interactive prompts.
//!
//! This shows how to:
//! 1. Register a UserInputHandler on a session
//! 2. Handle choice-based prompts from the agent (multiple choice)
//! 3. Handle freeform text input requests
//! 4. The agent can use the ask_user tool to request user input during execution

use copilot_sdk::{
    Client, PermissionRequestResult, SessionConfig, SessionEventData, UserInputRequest,
    UserInputResponse,
};
use std::io::{self, Write};

#[tokio::main]
async fn main() -> copilot_sdk::Result<()> {
    let client = Client::builder().build()?;

    println!("=== User Input Handler Example ===\n");
    println!("When the agent needs user input, it will invoke the ask_user tool.");
    println!("Your handler receives the question and optional choices,");
    println!("and returns the user's answer.\n");

    client.start().await?;

    let config = SessionConfig {
        request_user_input: Some(true),
        request_permission: Some(true),
        ..Default::default()
    };

    let session = client.create_session(config).await?;

    // Register permission handler (approve all)
    session
        .register_permission_handler(|_req| PermissionRequestResult::approved())
        .await;

    // Register user input handler
    session
        .register_user_input_handler(|request: &UserInputRequest, _inv| {
            println!("\n╔══════════════════════════════════════╗");
            println!("║       AGENT ASKS FOR INPUT           ║");
            println!("╚══════════════════════════════════════╝");
            println!("\nQuestion: {}", request.question);

            if let Some(choices) = &request.choices {
                if !choices.is_empty() {
                    println!("\nChoices:");
                    for (i, choice) in choices.iter().enumerate() {
                        println!("  [{}] {}", i + 1, choice);
                    }
                    print!("\nEnter choice number (or type a custom answer): ");
                    io::stdout().flush().unwrap();

                    let mut input = String::new();
                    io::stdin().read_line(&mut input).unwrap();
                    let input = input.trim().to_string();

                    // Try to parse as a number
                    if let Ok(choice) = input.parse::<usize>() {
                        if choice >= 1 && choice <= choices.len() {
                            let answer = choices[choice - 1].clone();
                            println!("Selected: {answer}");
                            return UserInputResponse {
                                answer,
                                was_freeform: Some(false),
                            };
                        }
                    }

                    // Treat as freeform input
                    return UserInputResponse {
                        answer: input,
                        was_freeform: Some(true),
                    };
                }
            }

            // Freeform input (no choices provided)
            print!("\nYour answer: ");
            io::stdout().flush().unwrap();

            let mut input = String::new();
            io::stdin().read_line(&mut input).unwrap();

            UserInputResponse {
                answer: input.trim().to_string(),
                was_freeform: Some(true),
            }
        })
        .await;

    // Send a prompt that should trigger user input
    let prompt = "Ask me what my favorite programming language is, then tell me something interesting about it.";
    println!("Sending: {prompt}\n");

    session.send(prompt).await?;

    // Process events
    let mut events = session.subscribe();
    while let Ok(event) = events.recv().await {
        match &event.data {
            SessionEventData::AssistantMessage(msg) => {
                println!("{}", msg.content);
            }
            SessionEventData::AssistantMessageDelta(delta) => {
                print!("{}", delta.delta_content);
                io::stdout().flush().unwrap();
            }
            SessionEventData::SessionIdle(_) => break,
            SessionEventData::SessionError(err) => {
                eprintln!("Error: {}", err.message);
                break;
            }
            _ => {}
        }
    }

    client.stop().await;
    println!("\nDone!");
    Ok(())
}
