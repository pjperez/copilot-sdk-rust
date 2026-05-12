# copilot-sdk (Rust)

Rust SDK for interacting with the GitHub Copilot CLI agent runtime (JSON-RPC over stdio or TCP).

This is a Rust port of the upstream SDKs and is currently in technical preview.

## Requirements

- Rust 1.85+ (Edition 2024)
- GitHub Copilot CLI installed and authenticated
- `copilot` available in `PATH`, or set `COPILOT_CLI_PATH` to the CLI executable/script

## Install

Once published, add:

```toml
[dependencies]
copilot-sdk = "0.1"
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
```

For development from this repository:

```toml
[dependencies]
copilot-sdk = { path = "." }
```

## Quick Start

```rust
use copilot_sdk::{Client, SessionConfig};

#[tokio::main]
async fn main() -> copilot_sdk::Result<()> {
    let client = Client::builder().build()?;
    client.start().await?;

    let session = client.create_session(SessionConfig::default()).await?;
    let response = session.send_and_collect("Hello!", None).await?;
    println!("{}", response);

    session.disconnect().await?;
    client.stop().await;
    Ok(())
}
```

## Features

### Custom Tools

Register tools that the assistant can invoke:

```rust
session.register_tool_with_handler(
    Tool::builder("get_weather", "Get current weather")
        .string_param("city", "City name", true)
        .build(),
    |invocation| async move {
        let city: String = invocation.arg("city")?;
        Ok(ToolResult::text(format!("Weather in {}: Sunny, 72°F", city)))
    },
).await;
```

### Slash Commands

Register slash commands that users can invoke:

```rust
use copilot_sdk::{CommandDefinition, CommandContext, CommandResult};
use std::sync::Arc;

let help_cmd = CommandDefinition {
    name: "help".to_string(),
    description: "Show available commands".to_string(),
    handler: Some(Arc::new(|_ctx: &CommandContext| CommandResult {
        message: Some("Available: /help, /status".to_string()),
        suppress: false,
    })),
};
session.register_command(help_cmd).await;
```

### Elicitation (Interactive UI Dialogs)

Handle interactive prompts from the CLI:

```rust
use copilot_sdk::{ElicitationHandler, ElicitationResult};
use std::sync::Arc;

let handler: ElicitationHandler = Arc::new(|ctx| {
    match ctx.params.elicitation_type.as_str() {
        "confirm" => ElicitationResult::accept(serde_json::json!(true)),
        _ => ElicitationResult::dismiss(),
    }
});
session.register_elicitation_handler(handler).await;
```

### Blob Attachments

Send inline data without files on disk:

```rust
use copilot_sdk::{UserMessageAttachment, MessageOptions};

let blob = UserMessageAttachment::blob("csv data here", "text/csv", "data.csv");
session.send(MessageOptions {
    prompt: "Analyze this data".to_string(),
    attachments: Some(vec![blob]),
    mode: None,
}).await?;
```

### Runtime Model Switching

Change the model used by a session at runtime:

```rust
session.set_model("gpt-4o", None, None).await?;
```

### Python SDK Parity Options

Session creation and resume support the same wire options as the Python SDK, including
client names, model capability overrides, per-session GitHub tokens, default-agent
configuration, agent selection, config discovery, sub-agent streaming controls,
per-message request headers, additional instruction directories, per-session telemetry,
`continuePendingWork` on resume, and the new exit-plan-mode / auto-mode-switch opt-in
flags:

```rust
let config = SessionConfig {
    client_name: Some("my-app".into()),
    include_sub_agent_streaming_events: Some(true),
    agent: Some("code-reviewer".into()),
    enable_config_discovery: Some(true),
    instruction_directories: Some(vec!["./prompts".into()]),
    enable_session_telemetry: Some(false),
    request_exit_plan_mode: Some(true),
    request_auto_mode_switch: Some(true),
    ..Default::default()
};

let capabilities = session.capabilities().await;
```

### Interactive UI Dialogs (`session.ui`)

Drive `confirm` / `select` / `input` / raw `elicitation` dialogs back to the CLI host:

```rust
let ui = session.ui();
if ui.confirm("Apply these changes?").await? {
    let env = ui.select("Pick environment", &["staging", "prod"]).await?;
    println!("deploying to {env:?}");
}
```

### Exit Plan Mode & Auto Mode Switch

Register handlers that the runtime can call when the agent wants to leave plan mode or
switch into auto mode after a rate limit:

```rust
session.register_exit_plan_mode_handler(Arc::new(|req| ExitPlanModeResult {
    approved: true,
    selected_action: Some(req.recommended_action.clone()),
    feedback: None,
})).await;

session.register_auto_mode_switch_handler(Arc::new(|_req| {
    AutoModeSwitchResponse::YesAlways
})).await;
```

### Slash Command Broadcast Dispatch

Slash commands registered via `Session::register_command` (or the
`SessionConfig.commands` field) are now invoked automatically when the runtime
broadcasts a `command.execute` event; the SDK responds via
`session.commands.handlePendingCommand` with the handler result.

### System Message Customize Mode

`SystemMessageMode::Customize` plus `SectionOverride { section, action }` lets you
replace, remove, append, prepend, or transform individual sections of the SDK-managed
prompt. `SectionOverrideAction::Transform(Arc<dyn Fn>)` callbacks are dispatched via the
runtime's `systemMessage.transform` RPC:

```rust
let config = SessionConfig {
    section_overrides: Some(vec![
        SectionOverride {
            section: SystemPromptSection::Tone,
            action: SectionOverrideAction::Replace("Be concise and technical.".into()),
        },
        SectionOverride {
            section: SystemPromptSection::Identity,
            action: SectionOverrideAction::Transform(Arc::new(|content| {
                format!("{content}\nAlso speak like a pirate.")
            })),
        },
    ]),
    ..Default::default()
};
```

### Server-Side Filesystem Provider (`SessionFsProvider`)

Implement [`SessionFsProvider`] to host a custom session-state filesystem that the
runtime calls back into. Wire it via `SessionConfig::create_session_fs_handler`:

```rust
let factory = CreateSessionFsHandler::new(|_session_id| -> SharedSessionFsProvider {
    Arc::new(MyFsBackend::new())
});

let session = client.create_session(SessionConfig {
    create_session_fs_handler: Some(factory),
    ..Default::default()
}).await?;
```

### Connect Handshake & TCP Connection Token

The client now sends the `connect` RPC during startup (with the optional
`tcp_connection_token` for TCP transports) and falls back to `ping` for legacy v2 servers.
TCP connection tokens are also exported as `COPILOT_CONNECTION_TOKEN` to spawned CLI
subprocesses; `copilot_home` and `remote` (Mission Control) are exposed on
[`ClientOptions`] / [`ClientBuilder`].

### Session List Filters & Metadata Context

[`SessionListFilter`] now mirrors the Python SDK's `cwd` / `git_root` / `repository` /
`branch` filter fields (in addition to the Rust-only `status` / `model` / `since` /
`before` / `limit`). [`SessionMetadata`] carries an optional `context: SessionContext`
with the working-directory and git context the session was created in.

### System Prompt Section Overrides

Customize system prompt sections:

```rust
use copilot_sdk::{SectionOverride, SectionOverrideAction, SystemPromptSection};

let config = SessionConfig {
    section_overrides: Some(vec![
        SectionOverride {
            section: SystemPromptSection::Tone,
            action: SectionOverrideAction::Replace("Be concise and technical.".into()),
        },
    ]),
    ..Default::default()
};
```

### Infinite Sessions

Automatic context window management that compacts conversation history when approaching token limits:

```rust
let config = SessionConfig {
    infinite_sessions: Some(InfiniteSessionConfig::enabled()),
    ..Default::default()
};
```

### Session Lifecycle Hooks

Intercept tool calls and session events:

```rust
let config = SessionConfig {
    hooks: Some(SessionHooks {
        on_pre_tool_use: Some(Arc::new(|input| {
            if input.tool_name == "dangerous_tool" {
                return PreToolUseHookOutput {
                    permission_decision: Some("deny".into()),
                    ..Default::default()
                };
            }
            PreToolUseHookOutput::default()
        })),
        ..Default::default()
    }),
    ..Default::default()
};
```

### Client Utilities

```rust
let status = client.get_status().await?;       // CLI version info
let auth = client.get_auth_status().await?;    // Authentication state
let models = client.list_models().await?;      // Available models
let sessions = client.list_sessions(None).await?;  // Active sessions
```

### BYOK (Bring Your Own Key)

Use your own API keys with compatible providers:

```rust
let config = SessionConfig {
    provider: Some(ProviderConfig {
        base_url: Some("https://api.openai.com/v1".into()),
        api_key: Some("sk-...".into()),
        ..Default::default()
    }),
    ..Default::default()
};
```

## Examples

```bash
cargo run --example basic_chat          # Simple chat
cargo run --example tool_usage          # Tool registration
cargo run --example fluent_tools        # Builder-pattern tools
cargo run --example streaming           # Streaming events
cargo run --example commands            # Slash commands
cargo run --example elicitation         # Interactive UI dialogs
cargo run --example session_ui          # confirm/select/input via session.ui
cargo run --example exit_plan_mode      # exitPlanMode + autoModeSwitch handlers
cargo run --example session_fs_provider # In-memory SessionFsProvider
cargo run --example blob_attachments    # Inline data attachments
cargo run --example set_model           # Runtime model switching
cargo run --example hooks               # Lifecycle hooks
cargo run --example attachments         # File attachments
cargo run --example external_server     # External TCP server
```

## Development

### Setup

Enable pre-commit hooks to catch formatting/linting issues before push:

```bash
git config core.hooksPath .githooks
```

### Commands

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

E2E tests (real Copilot CLI):

```bash
cargo test --features e2e -- --test-threads=1
```

Snapshot conformance tests (optional, against upstream YAML snapshots):

```bash
cargo test --features snapshots --test snapshot_conformance
```

Set `COPILOT_SDK_RUST_SNAPSHOT_DIR` or `UPSTREAM_SNAPSHOTS` to point at `copilot-sdk/test/snapshots` if it cannot be auto-detected.

## Notes

- Supports stdio (spawned CLI) and TCP (spawned or external server).

## License

MIT License - see [LICENSE](LICENSE).

## Related

- Upstream SDKs: https://github.com/github/copilot-sdk
