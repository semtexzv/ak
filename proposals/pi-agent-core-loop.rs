// =============================================================================
// Pi Agent Core Loop — Rust Proposal
// =============================================================================
//
// A synchronous, single-threaded agent core loop inspired by pi-agent-core
// (badlogic/pi-mono). No async in the core loop. Extension via traits and
// closures with zero mandatory allocations in the hot path.
//
// Design goals:
//   1. Synchronous core — the loop is a plain `while` loop, no executor needed.
//   2. Transport-agnostic — callers supply a `Transport` impl (HTTP, mock, etc.)
//   3. Extensible tools — tools are trait objects with schema-driven validation.
//   4. Event-driven observability — every state transition emits an `Event`.
//   5. Interruptible — steering messages can abort in-flight tool execution.
//   6. Serialisable state — `AgentState` is `Serialize + Deserialize` for
//      session persistence, branching, and restore.
//
// The file is structured as a single-crate sketch. Production code would split
// this across modules (types.rs, tool.rs, transport.rs, loop.rs, agent.rs).

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::sync::mpsc;
use std::time::Instant;

// ---------------------------------------------------------------------------
// 1. Content & Message types
// ---------------------------------------------------------------------------

/// A content block inside a message — mirrors LLM content arrays.
#[derive(Debug, Clone)]
pub enum ContentBlock {
    Text(String),
    ToolCall(ToolCall),
    ToolResult(ToolResult),
    /// Extension point: embedders can smuggle domain-specific blocks through.
    Custom(Box<dyn CustomContent>),
}

/// Trait for user-defined content blocks (images, documents, …).
pub trait CustomContent: fmt::Debug + Send + 'static {
    fn type_name(&self) -> &str;
    fn clone_box(&self) -> Box<dyn CustomContent>;
}

impl Clone for Box<dyn CustomContent> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

#[derive(Debug, Clone)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// JSON-encoded arguments.
    pub arguments: String,
}

#[derive(Debug, Clone)]
pub struct ToolResult {
    pub tool_call_id: String,
    pub content: Vec<ContentBlock>,
    pub is_error: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

impl Message {
    pub fn user(text: impl Into<String>) -> Self {
        Message {
            role: Role::User,
            content: vec![ContentBlock::Text(text.into())],
        }
    }

    pub fn tool_calls(&self) -> Vec<&ToolCall> {
        self.content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::ToolCall(tc) => Some(tc),
                _ => None,
            })
            .collect()
    }

    pub fn has_tool_calls(&self) -> bool {
        self.content
            .iter()
            .any(|b| matches!(b, ContentBlock::ToolCall(_)))
    }
}

// ---------------------------------------------------------------------------
// 2. Stop reason
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    Error,
    Aborted,
}

// ---------------------------------------------------------------------------
// 3. Transport — the only I/O boundary
// ---------------------------------------------------------------------------

/// Response from the LLM after a single inference call.
pub struct LlmResponse {
    pub message: Message,
    pub stop_reason: StopReason,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// The transport trait abstracts the LLM call. Implementations can be
/// synchronous HTTP calls, mock responses, or thin wrappers around async
/// runtimes (`block_on`). The core loop never touches a runtime.
pub trait Transport {
    /// Send the full message history and get back a single assistant response.
    /// Called synchronously — if the implementation needs async, it blocks
    /// internally (e.g. via `tokio::runtime::Handle::block_on`).
    fn send(
        &self,
        system: &str,
        messages: &[Message],
        tools: &[ToolSchema],
        abort: &AbortFlag,
    ) -> Result<LlmResponse, TransportError>;
}

#[derive(Debug)]
pub enum TransportError {
    Network(String),
    RateLimit { retry_after_ms: u64 },
    Auth(String),
    Other(String),
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TransportError::Network(e) => write!(f, "network: {e}"),
            TransportError::RateLimit { retry_after_ms } => {
                write!(f, "rate limited, retry after {retry_after_ms}ms")
            }
            TransportError::Auth(e) => write!(f, "auth: {e}"),
            TransportError::Other(e) => write!(f, "{e}"),
        }
    }
}

// ---------------------------------------------------------------------------
// 4. Tool system
// ---------------------------------------------------------------------------

/// JSON Schema fragment describing a tool's parameters. In production this
/// would be a proper JSON Schema type; here we use a string for brevity.
#[derive(Debug, Clone)]
pub struct ToolSchema {
    pub name: String,
    pub description: String,
    /// JSON Schema for the `arguments` object.
    pub parameters_schema: String,
}

/// Result of a single tool execution.
pub struct ToolExecResult {
    pub content: Vec<ContentBlock>,
    pub is_error: bool,
}

impl ToolExecResult {
    pub fn text(s: impl Into<String>) -> Self {
        ToolExecResult {
            content: vec![ContentBlock::Text(s.into())],
            is_error: false,
        }
    }

    pub fn error(s: impl Into<String>) -> Self {
        ToolExecResult {
            content: vec![ContentBlock::Text(s.into())],
            is_error: true,
        }
    }
}

/// A tool that the agent can invoke. Implementations are synchronous — if a
/// tool needs to run a subprocess, it blocks. This keeps the trait trivial
/// to implement and test.
pub trait Tool: Send {
    fn schema(&self) -> ToolSchema;

    /// Execute the tool. `arguments` is the raw JSON string from the model.
    /// The `abort` flag should be polled periodically in long-running tools.
    fn execute(
        &self,
        call_id: &str,
        arguments: &str,
        abort: &AbortFlag,
    ) -> ToolExecResult;
}

// ---------------------------------------------------------------------------
// 5. Abort flag
// ---------------------------------------------------------------------------

/// A thread-safe flag for cooperative cancellation. Checked by tools and the
/// transport during long-running operations.
pub struct AbortFlag {
    inner: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl AbortFlag {
    pub fn new() -> Self {
        AbortFlag {
            inner: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    pub fn abort(&self) {
        self.inner
            .store(true, std::sync::atomic::Ordering::Release);
    }

    pub fn is_aborted(&self) -> bool {
        self.inner.load(std::sync::atomic::Ordering::Acquire)
    }

    pub fn reset(&self) {
        self.inner
            .store(false, std::sync::atomic::Ordering::Release);
    }

    pub fn clone_flag(&self) -> Self {
        AbortFlag {
            inner: self.inner.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// 6. Events
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum Event {
    AgentStart,
    TurnStart { turn: u32 },
    /// The model produced a response.
    AssistantMessage { message: Message },
    ToolExecStart { call_id: String, name: String },
    ToolExecEnd { call_id: String, result: ToolResult },
    /// A steering message was injected mid-turn.
    Steered { message: Message },
    TurnEnd { turn: u32 },
    AgentEnd { reason: AgentEndReason },
    /// Token accounting per turn.
    TokenUsage { input: u64, output: u64 },
}

#[derive(Debug)]
pub enum AgentEndReason {
    /// Model stopped on its own (no more tool calls).
    Done,
    /// Aborted via the abort flag.
    Aborted,
    /// Unrecoverable transport error.
    Error(String),
}

/// Sink for events. Implementations can log, update a TUI, send over a
/// channel, or aggregate metrics. Multiple sinks can be composed.
pub trait EventSink: Send {
    fn on_event(&mut self, event: &Event);
}

/// Trivial sink that discards everything.
pub struct NullSink;
impl EventSink for NullSink {
    fn on_event(&mut self, _: &Event) {}
}

/// Sink that forwards to an `mpsc::Sender`.
pub struct ChannelSink(pub mpsc::Sender<Event>);
// (Event would need Clone for this; omitted for sketch clarity.)

// ---------------------------------------------------------------------------
// 7. Agent state (serialisable core)
// ---------------------------------------------------------------------------

/// Everything needed to freeze and restore a session. The `messages` vec is
/// the conversation history; `pending_tool_calls` is normally empty at rest.
#[derive(Debug, Clone)]
pub struct AgentState {
    pub system_prompt: String,
    pub messages: Vec<Message>,
    pub turn: u32,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
}

impl AgentState {
    pub fn new(system_prompt: impl Into<String>) -> Self {
        AgentState {
            system_prompt: system_prompt.into(),
            messages: Vec::new(),
            turn: 0,
            total_input_tokens: 0,
            total_output_tokens: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// 8. Message queues (steering & follow-up)
// ---------------------------------------------------------------------------

/// Two-queue system for injecting messages into the loop at safe points.
///
/// - **Steering**: delivered after the current tool finishes; remaining
///   queued tool calls in the batch are skipped.
/// - **Follow-up**: delivered only when the agent would otherwise stop
///   (no pending tool calls, no steering messages).
pub struct MessageQueues {
    steering: VecDeque<Message>,
    follow_up: VecDeque<Message>,
}

impl MessageQueues {
    pub fn new() -> Self {
        MessageQueues {
            steering: VecDeque::new(),
            follow_up: VecDeque::new(),
        }
    }

    /// Interrupt the current turn with a message.
    pub fn steer(&mut self, msg: Message) {
        self.steering.push_back(msg);
    }

    /// Enqueue a message for after the current agent invocation finishes.
    pub fn follow_up(&mut self, msg: Message) {
        self.follow_up.push_back(msg);
    }

    fn take_steering(&mut self) -> Option<Message> {
        self.steering.pop_front()
    }

    fn take_follow_up(&mut self) -> Option<Message> {
        self.follow_up.pop_front()
    }

    fn has_steering(&self) -> bool {
        !self.steering.is_empty()
    }

    fn has_follow_up(&self) -> bool {
        !self.follow_up.is_empty()
    }
}

// ---------------------------------------------------------------------------
// 9. Context transform hook
// ---------------------------------------------------------------------------

/// Called before each LLM call, allowing callers to compact, summarise, or
/// otherwise transform the message history. This is where context-window
/// management lives — outside the core loop.
pub trait ContextTransform: Send {
    fn transform(&self, system: &str, messages: &[Message]) -> (String, Vec<Message>);
}

/// Identity transform — pass everything through unchanged.
pub struct NoTransform;
impl ContextTransform for NoTransform {
    fn transform(&self, system: &str, messages: &[Message]) -> (String, Vec<Message>) {
        (system.to_string(), messages.to_vec())
    }
}

// ---------------------------------------------------------------------------
// 10. Agent configuration (builder)
// ---------------------------------------------------------------------------

pub struct AgentConfig {
    pub tools: Vec<Box<dyn Tool>>,
    pub transport: Box<dyn Transport>,
    pub event_sink: Box<dyn EventSink>,
    pub context_transform: Box<dyn ContextTransform>,
    /// Max consecutive transport errors before giving up. 0 = unlimited.
    pub max_transport_errors: u32,
}

// ---------------------------------------------------------------------------
// 11. THE CORE LOOP
// ---------------------------------------------------------------------------

/// Run the agent loop synchronously.
///
/// This is the heart of the system: a plain `while` loop with no async, no
/// executor, no hidden concurrency. The only blocking calls are inside
/// `transport.send()` and `tool.execute()` — both are synchronous by contract.
///
/// ```text
///  ┌────────────────────────────────────────────────────┐
///  │                    OUTER LOOP                      │
///  │  (continues while follow-up messages exist)        │
///  │  ┌──────────────────────────────────────────────┐  │
///  │  │               INNER LOOP                     │  │
///  │  │  1. Send messages → LLM (via Transport)      │  │
///  │  │  2. Collect tool calls from response          │  │
///  │  │  3. Execute each tool (check steering/abort)  │  │
///  │  │  4. Feed results back into messages           │  │
///  │  │  5. If no tool calls → break inner loop       │  │
///  │  └──────────────────────────────────────────────┘  │
///  │  Check for follow-up messages; if any, continue    │
///  └────────────────────────────────────────────────────┘
/// ```
pub fn agent_loop(
    initial_messages: Vec<Message>,
    state: &mut AgentState,
    queues: &mut MessageQueues,
    config: &AgentConfig,
    abort: &AbortFlag,
) -> AgentEndReason {
    // Build tool schema table once.
    let tool_schemas: Vec<ToolSchema> = config.tools.iter().map(|t| t.schema()).collect();
    let tool_map: HashMap<&str, &dyn Tool> = config
        .tools
        .iter()
        .map(|t| (t.schema().name.as_str(), t.as_ref()))
        .collect();

    // Append initial user messages.
    state.messages.extend(initial_messages);

    config.event_sink_emit(&Event::AgentStart);

    let mut consecutive_errors: u32 = 0;

    // ── OUTER LOOP ──────────────────────────────────────────────────────
    'outer: loop {
        state.turn += 1;
        config.event_sink_emit(&Event::TurnStart { turn: state.turn });

        // ── INNER LOOP ──────────────────────────────────────────────────
        'inner: loop {
            if abort.is_aborted() {
                config.event_sink_emit(&Event::AgentEnd {
                    reason: AgentEndReason::Aborted,
                });
                return AgentEndReason::Aborted;
            }

            // 1. Optional context transform (compaction, summarisation).
            let (sys, msgs) = config
                .context_transform
                .transform(&state.system_prompt, &state.messages);

            // 2. Call the LLM.
            let response = match config.transport.send(&sys, &msgs, &tool_schemas, abort) {
                Ok(r) => {
                    consecutive_errors = 0;
                    r
                }
                Err(e) => {
                    consecutive_errors += 1;
                    if config.max_transport_errors > 0
                        && consecutive_errors >= config.max_transport_errors
                    {
                        let reason = AgentEndReason::Error(format!(
                            "transport failed {consecutive_errors} times: {e}"
                        ));
                        config.event_sink_emit(&Event::AgentEnd {
                            reason: AgentEndReason::Error(e.to_string()),
                        });
                        return reason;
                    }
                    // Retry: just loop again. Callers can implement backoff
                    // in the Transport itself.
                    continue 'inner;
                }
            };

            // Track tokens.
            state.total_input_tokens += response.input_tokens;
            state.total_output_tokens += response.output_tokens;
            config.event_sink_emit(&Event::TokenUsage {
                input: response.input_tokens,
                output: response.output_tokens,
            });

            // 3. Record assistant message.
            config.event_sink_emit(&Event::AssistantMessage {
                message: response.message.clone(),
            });
            state.messages.push(response.message.clone());

            // 4. Extract tool calls.
            let tool_calls = response.message.tool_calls();

            if tool_calls.is_empty() {
                // No tool calls → this turn is done.
                break 'inner;
            }

            if response.stop_reason == StopReason::Error
                || response.stop_reason == StopReason::Aborted
            {
                break 'inner;
            }

            // 5. Execute tool calls, respecting steering/abort.
            let mut steered = false;
            for tc in &tool_calls {
                // Check for steering message before each tool.
                if let Some(steer_msg) = queues.take_steering() {
                    config.event_sink_emit(&Event::Steered {
                        message: steer_msg.clone(),
                    });
                    state.messages.push(steer_msg);
                    steered = true;

                    // Skip remaining tools — push error results for unskipped.
                    push_skipped_tool_result(state, tc);
                    continue;
                }

                if steered {
                    push_skipped_tool_result(state, tc);
                    continue;
                }

                if abort.is_aborted() {
                    push_skipped_tool_result(state, tc);
                    continue;
                }

                config.event_sink_emit(&Event::ToolExecStart {
                    call_id: tc.id.clone(),
                    name: tc.name.clone(),
                });

                let result = match tool_map.get(tc.name.as_str()) {
                    Some(tool) => tool.execute(&tc.id, &tc.arguments, abort),
                    None => ToolExecResult::error(format!("unknown tool: {}", tc.name)),
                };

                let tool_result = ToolResult {
                    tool_call_id: tc.id.clone(),
                    content: result.content,
                    is_error: result.is_error,
                };

                config.event_sink_emit(&Event::ToolExecEnd {
                    call_id: tc.id.clone(),
                    result: tool_result.clone(),
                });

                // Feed result back into conversation.
                state.messages.push(Message {
                    role: Role::User,
                    content: vec![ContentBlock::ToolResult(tool_result)],
                });
            }

            // Continue inner loop — the LLM will see tool results next turn.
        }
        // ── END INNER LOOP ──────────────────────────────────────────────

        config.event_sink_emit(&Event::TurnEnd { turn: state.turn });

        // Check for follow-up messages before declaring done.
        if let Some(follow_up) = queues.take_follow_up() {
            state.messages.push(follow_up);
            continue 'outer;
        }

        // Also check steering queue (could have arrived during last tool exec).
        if let Some(steer_msg) = queues.take_steering() {
            state.messages.push(steer_msg);
            continue 'outer;
        }

        // Nothing left — we're done.
        config.event_sink_emit(&Event::AgentEnd {
            reason: AgentEndReason::Done,
        });
        return AgentEndReason::Done;
    }
}

/// Helper: push a "skipped" tool result so the LLM knows the tool wasn't run.
fn push_skipped_tool_result(state: &mut AgentState, tc: &ToolCall) {
    state.messages.push(Message {
        role: Role::User,
        content: vec![ContentBlock::ToolResult(ToolResult {
            tool_call_id: tc.id.clone(),
            content: vec![ContentBlock::Text(
                "Tool execution skipped (steering message or abort)".into(),
            )],
            is_error: true,
        })],
    });
}

// Convenience method to avoid borrow-checker friction with &mut event_sink.
impl AgentConfig {
    fn event_sink_emit(&self, _event: &Event) {
        // In production: self.event_sink.on_event(event)
        // Sketched as no-op because `&self` vs `&mut self` on event_sink
        // needs interior mutability (RefCell/Mutex). See note below.
    }
}

// ---------------------------------------------------------------------------
// 12. Extension semantics — how to add capabilities
// ---------------------------------------------------------------------------

// The extension surface is deliberately minimal. There are exactly three
// extension points, each a trait:
//
// ┌─────────────────────────────────────────────────────────────────┐
// │  Extension Point    │ Trait           │ Purpose                 │
// ├─────────────────────┼─────────────────┼─────────────────────────┤
// │  What the model     │ Tool            │ New capabilities        │
// │  can do             │                 │ (bash, read, write, …)  │
// ├─────────────────────┼─────────────────┼─────────────────────────┤
// │  How the model is   │ Transport       │ Provider, retry, cache  │
// │  called             │                 │                         │
// ├─────────────────────┼─────────────────┼─────────────────────────┤
// │  What the caller    │ EventSink       │ TUI, logging, metrics   │
// │  sees               │                 │                         │
// ├─────────────────────┼─────────────────┼─────────────────────────┤
// │  Context management │ ContextTransform│ Compaction, summarise   │
// │                     │                 │                         │
// └─────────────────────┴─────────────────┴─────────────────────────┘
//
// Everything else (session persistence, branching, UI, skills) is built
// *on top* of these primitives. The core loop doesn't know about any of it.

// ---------------------------------------------------------------------------
// 13. Example: a minimal tool implementation
// ---------------------------------------------------------------------------

/// Example: a tool that reads a file from disk.
pub struct ReadFileTool;

impl Tool for ReadFileTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "read".into(),
            description: "Read a file from disk".into(),
            parameters_schema: r#"{
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Absolute file path" }
                },
                "required": ["path"]
            }"#
            .into(),
        }
    }

    fn execute(&self, _call_id: &str, arguments: &str, _abort: &AbortFlag) -> ToolExecResult {
        // In production: parse JSON, validate, read file, handle errors.
        // Sketch:
        let path = arguments; // pretend we parsed it
        match std::fs::read_to_string(path) {
            Ok(content) => ToolExecResult::text(content),
            Err(e) => ToolExecResult::error(format!("failed to read {path}: {e}")),
        }
    }
}

// ---------------------------------------------------------------------------
// 14. Example: composing an event sink
// ---------------------------------------------------------------------------

/// Sink that writes a structured log line for every event.
pub struct LogSink;

impl EventSink for LogSink {
    fn on_event(&mut self, event: &Event) {
        eprintln!("[agent] {event:?}");
    }
}

/// Combine multiple sinks.
pub struct CompositeSink(Vec<Box<dyn EventSink>>);

impl EventSink for CompositeSink {
    fn on_event(&mut self, event: &Event) {
        for sink in &mut self.0 {
            sink.on_event(event);
        }
    }
}

// ---------------------------------------------------------------------------
// 15. Example: putting it all together
// ---------------------------------------------------------------------------

#[cfg(never)] // conceptual — won't compile without a real Transport impl
fn example_usage() {
    let mut state = AgentState::new("You are a helpful coding assistant.");
    let mut queues = MessageQueues::new();
    let abort = AbortFlag::new();

    let config = AgentConfig {
        tools: vec![Box::new(ReadFileTool)],
        transport: todo!("provide an impl"),
        event_sink: Box::new(LogSink),
        context_transform: Box::new(NoTransform),
        max_transport_errors: 3,
    };

    let reason = agent_loop(
        vec![Message::user("Read the file at /tmp/hello.txt and summarise it.")],
        &mut state,
        &mut queues,
        &config,
        &abort,
    );

    match reason {
        AgentEndReason::Done => println!("Agent finished."),
        AgentEndReason::Aborted => println!("Agent was aborted."),
        AgentEndReason::Error(e) => eprintln!("Agent error: {e}"),
    }

    // State can be serialised and restored for session persistence.
    println!("Tokens used: {} in, {} out", state.total_input_tokens, state.total_output_tokens);
}

// ---------------------------------------------------------------------------
// 16. Design notes
// ---------------------------------------------------------------------------

// WHY NO ASYNC IN THE CORE LOOP?
//
// The core loop is a state machine driven by two blocking operations:
// Transport::send() and Tool::execute(). Making these async would:
//   - Force every tool author to deal with async (Pin, Send, lifetime soup).
//   - Require an executor in scope — coupling the core to tokio/smol/etc.
//   - Gain almost nothing: the loop is fundamentally sequential. The model
//     responds, we execute tools one by one, repeat. There's no useful
//     concurrency to exploit *within* a single turn.
//
// If callers need to run the loop off the main thread or integrate with an
// async runtime, they spawn a thread:
//
//   std::thread::spawn(move || agent_loop(msgs, &mut state, ...));
//
// Or use `spawn_blocking` in tokio. The core stays simple.
//
//
// PARALLEL TOOL EXECUTION (optional, outside core):
//
// When the model emits N tool calls, pi executes them sequentially (with
// steering checks between each). If you want parallel tool execution, wrap
// tools in a `ParallelToolExecutor` that spawns threads internally but
// presents a synchronous interface to the loop. This keeps the core simple
// while allowing callers to opt into concurrency.
//
//
// EVENT SINK & INTERIOR MUTABILITY:
//
// The EventSink takes `&mut self` for simplicity, but the core loop holds
// AgentConfig by `&`. In production, use one of:
//   - `RefCell<Box<dyn EventSink>>` (single-threaded)
//   - `Mutex<Box<dyn EventSink>>` (multi-threaded)
//   - A channel-based sink that takes `&self` (no mutation needed)
//
// The channel approach is cleanest: `impl EventSink for mpsc::Sender<Event>`.
//
//
// EXTENSION SEMANTICS — WHY TRAITS, NOT CLOSURES:
//
// Traits over closures for Tool/Transport/EventSink because:
//   1. Named types are self-documenting and testable.
//   2. Trait objects compose (CompositeSink, FallbackTransport).
//   3. Schema introspection (Tool::schema()) doesn't fit closures naturally.
//   4. Downstream crates can implement traits without depending on our crate's
//      closure signatures.
//
// Closures are still useful for one-off tools in tests:
//
//   struct FnTool<F: Fn(&str, &str, &AbortFlag) -> ToolExecResult> {
//       name: String,
//       schema: ToolSchema,
//       f: F,
//   }
//
//
// SESSION BRANCHING:
//
// Since AgentState is Clone, branching is trivial:
//
//   let branch = state.clone();
//   agent_loop(side_quest_msgs, &mut branch, ...);
//   // branch diverges; original state is unchanged.
//
// This mirrors pi's session tree model.

fn main() {
    // This file is a design proposal, not a runnable binary.
    println!("See the code and comments above for the proposed architecture.");
}
