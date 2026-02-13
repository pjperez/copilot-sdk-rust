// Copyright (c) 2026 Elias Bachaalany
// SPDX-License-Identifier: MIT

//! Deny/Allow Tools example - configuring tool access policies via the CLI.
//!
//! This example demonstrates:
//! - Using `deny_tool()` to block specific shell commands
//! - Using `allow_tool()` to pre-approve safe commands
//! - Using `allow_all_tools(true)` with selective denials
//! - How deny takes precedence over allow
//!
//! The deny/allow tool options are passed as `--deny-tool` / `--allow-tool` /
//! `--allow-all-tools` arguments to the Copilot CLI, which enforces them at
//! the CLI level before the SDK even sees a permission request.

use copilot_sdk::{Client, PermissionRequestResult, SessionConfig};

#[tokio::main]
async fn main() -> copilot_sdk::Result<()> {
    println!("=== Deny/Allow Tools Example ===\n");

    // Example 1: Allow everything EXCEPT dangerous git operations
    println!("--- Example 1: Allow-all with exceptions ---");
    let client = Client::builder()
        .use_stdio(true)
        .allow_all_tools(true)
        .deny_tool("shell(git push)")
        .deny_tool("shell(git commit)")
        .deny_tool("shell(rm)")
        .build()?;
    client.start().await?;

    let session = client.create_session(SessionConfig::default()).await?;

    println!("Session: {}", session.session_id());
    println!("Policy: allow all tools, deny git push/commit and rm");

    // Permission handler still fires for anything not auto-approved
    session
        .register_permission_handler(|req| {
            let tool = req
                .extension_data
                .get("toolName")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            println!("[Permission] {} -> approved", tool);
            PermissionRequestResult::approved()
        })
        .await;

    let response = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        session.send_and_collect("List the files in the current directory.", None),
    )
    .await
    .expect("Timeout")
    .expect("Failed to get response");

    println!("Response: {}\n", response);

    client.stop().await;

    // Example 2: Selective allow-list (only safe read operations)
    println!("--- Example 2: Selective allow-list ---");
    let client2 = Client::builder()
        .use_stdio(true)
        .allow_tool("shell(ls)")
        .allow_tool("shell(cat)")
        .allow_tool("shell(echo)")
        .build()?;
    client2.start().await?;

    let session2 = client2.create_session(SessionConfig::default()).await?;

    println!("Session: {}", session2.session_id());
    println!("Policy: only allow ls, cat, echo");

    session2
        .register_permission_handler(|req| {
            let tool = req
                .extension_data
                .get("toolName")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            println!("[Permission] {} -> approved (fallback)", tool);
            PermissionRequestResult::approved()
        })
        .await;

    let response2 = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        session2.send_and_collect("Run 'echo Hello World' for me.", None),
    )
    .await
    .expect("Timeout")
    .expect("Failed to get response");

    println!("Response: {}\n", response2);

    client2.stop().await;

    // Example 3: Using batch methods
    println!("--- Example 3: Batch deny/allow ---");
    let client3 = Client::builder()
        .use_stdio(true)
        .deny_tools(vec![
            "shell(git push)",
            "shell(git commit)",
            "shell(rm)",
            "write",
        ])
        .allow_tools(vec!["shell(ls)", "shell(cat)", "shell(echo)"])
        .build()?;
    client3.start().await?;

    println!("Policy: deny git push/commit, rm, write; allow ls, cat, echo");

    let session3 = client3.create_session(SessionConfig::default()).await?;

    session3
        .register_permission_handler(|_| PermissionRequestResult::approved())
        .await;

    let response3 = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        session3.send_and_collect("What is 2 + 2?", None),
    )
    .await
    .expect("Timeout")
    .expect("Failed");

    println!("Response: {}\n", response3);

    client3.stop().await;

    println!("Done!");
    Ok(())
}
