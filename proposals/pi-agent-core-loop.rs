// =============================================================================
// Pi Agent Core Loop — Rust Proposal (v2: Cooperative State Machine)
// =============================================================================
//
// Revision of the original blocking-loop design. The core loop is now a
// **cooperative state machine** that never blocks. It yields `Effect` values
// at every I/O boundary — the caller (the "driver") performs the actual I/O,
// updates the UI, checks for interrupts, and feeds results back.
//
// This gives the caller full control over:
//   - When and how I/O happens (sync, async, threaded, polled)
//   - UI updates between every interaction point
//   - Interrupt/steering injection at any yield point
//   - Streaming token display during LLM calls
//   - Parallel tool execution (caller's choice, not baked in)
//
// The core loop itself: zero I/O, zero blocking, zero async, zero threads.
// Pure data in → data out.
//
// ┌─────────────────────────────────────────────────────────────────────┐
// │                         ARCHITECTURE                               │
// │                                                                    │
// │   ┌──────────────┐  Effect   ┌──────────────┐                     │
// │   │              │ ────────> │              │                     │
// │   │  AgentLoop   │           │    Driver    │  ← caller code      │
// │   │  (pure FSM)  │ <──────── │  (your code) │                     │
// │   │              │  Resume   │              │                     │
// │   └──────────────┘           └──────┬───────┘                     │
// │         │                           │                             │
// │         │ state                     │ performs I/O, UI, etc.      │
// │         ▼                           ▼                             │
// │   ┌──────────────┐           ┌──────────────┐                     │
// │   │  AgentState   │           │  Transport   │                     │
// │   │  (serialise)  │           │  Tools       │                     │
// │   └──────────────┘           │  TUI/GUI     │                     │
// │                              └──────────────┘                     │
// └─────────────────────────────────────────────────────────────────────┘

use std::collections::VecDeque;
use std::fmt;

// ═══════════════════════════════════════════════════════════════════════
// PART 1: Data types (same as v1, included for completeness)
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub enum ContentBlock {
    Text(String),
    ToolCall(ToolCall),
    ToolResult(ToolResult),
}

#[derive(Debug, Clone)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
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

    pub fn tool_result(call_id: impl Into<String>, content: Vec<ContentBlock>, is_error: bool) -> Self {
        Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult(ToolResult {
                tool_call_id: call_id.into(),
                content,
                is_error,
            })],
        }
    }

    fn tool_calls(&self) -> impl Iterator<Item = &ToolCall> {
        self.content.iter().filter_map(|b| match b {
            ContentBlock::ToolCall(tc) => Some(tc),
            _ => None,
        })
    }

    fn has_tool_calls(&self) -> bool {
        self.content
            .iter()
            .any(|b| matches!(b, ContentBlock::ToolCall(_)))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    Error,
}

#[derive(Debug, Clone)]
pub struct ToolSchema {
    pub name: String,
    pub description: String,
    pub parameters_schema: String,
}

// ═══════════════════════════════════════════════════════════════════════
// PART 2: The Effect/Resume protocol — the core abstraction
// ═══════════════════════════════════════════════════════════════════════

/// What the loop needs the outside world to do.
///
/// The caller receives an `Effect`, performs the actual work (I/O, UI update,
/// tool execution, …) however it wants, then calls the corresponding
/// `resume_*` method to advance the state machine.
#[derive(Debug)]
pub enum Effect {
    /// The loop needs an LLM inference call.
    ///
    /// Caller should:
    ///   1. Optionally transform/compact the context
    ///   2. Send to the LLM (streaming or not — caller's choice)
    ///   3. While waiting: update UI, check for user interrupts
    ///   4. Call `resume_llm()` with the final response
    ///
    /// The request borrows nothing — caller owns it fully.
    NeedLlm(LlmRequest),

    /// The loop needs a tool executed.
    ///
    /// Caller should:
    ///   1. Look up the tool by name
    ///   2. Execute it (in-thread, spawned, pooled — caller's choice)
    ///   3. While waiting: update UI, check for user interrupts
    ///   4. Call `resume_tool()` with the result
    NeedToolExec(ToolCallRequest),

    /// An observable event happened inside the state machine.
    ///
    /// Caller should handle it (log, update UI, send metrics, …) then call
    /// `resume()` to continue. Events never require a response value — they
    /// are pure notifications.
    Event(Event),

    /// The loop is finished. Terminal state — no more resume calls needed.
    Done(EndReason),
}

/// Data needed to make an LLM call. Fully owned — the caller can mutate,
/// cache, retry, or discard it.
#[derive(Debug, Clone)]
pub struct LlmRequest {
    pub system_prompt: String,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSchema>,
}

/// Data needed to execute a tool.
#[derive(Debug, Clone)]
pub struct ToolCallRequest {
    pub call_id: String,
    pub name: String,
    pub arguments: String,
}

/// Result of an LLM call, provided by the caller.
pub struct LlmResponse {
    pub message: Message,
    pub stop_reason: StopReason,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// Result of a tool execution, provided by the caller.
pub struct ToolExecResult {
    pub content: Vec<ContentBlock>,
    pub is_error: bool,
}

impl ToolExecResult {
    pub fn text(s: impl Into<String>) -> Self {
        Self { content: vec![ContentBlock::Text(s.into())], is_error: false }
    }
    pub fn error(s: impl Into<String>) -> Self {
        Self { content: vec![ContentBlock::Text(s.into())], is_error: true }
    }
}

/// Transport error reported by the caller.
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
            Self::Network(e) => write!(f, "network: {e}"),
            Self::RateLimit { retry_after_ms } => write!(f, "rate limited ({retry_after_ms}ms)"),
            Self::Auth(e) => write!(f, "auth: {e}"),
            Self::Other(e) => write!(f, "{e}"),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// PART 3: Events — observability without coupling
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug)]
pub enum Event {
    AgentStart,
    TurnStart { turn: u32 },
    AssistantMessage { message: Message },
    ToolExecStart { call_id: String, name: String },
    ToolExecEnd { call_id: String, result: ToolResult },
    Steered { message: Message },
    TurnEnd { turn: u32 },
    TokenUsage { input: u64, output: u64 },
}

#[derive(Debug)]
pub enum EndReason {
    Done,
    Aborted,
    Error(String),
}

// ═══════════════════════════════════════════════════════════════════════
// PART 4: Agent state (serialisable, cloneable, branchable)
// ═══════════════════════════════════════════════════════════════════════

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
        Self {
            system_prompt: system_prompt.into(),
            messages: Vec::new(),
            turn: 0,
            total_input_tokens: 0,
            total_output_tokens: 0,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// PART 5: The state machine — the actual core loop
// ═══════════════════════════════════════════════════════════════════════

/// Internal phase of the state machine.
#[derive(Debug)]
enum Phase {
    /// Just started, or processing events before first LLM call.
    Init,
    /// Yielded NeedLlm, waiting for resume_llm().
    WaitingForLlm,
    /// Yielded NeedToolExec, waiting for resume_tool().
    WaitingForTool,
    /// Yielded Event, waiting for resume().
    WaitingForEventAck,
    /// Terminal.
    Done,
}

pub struct AgentLoop {
    state: AgentState,
    tool_schemas: Vec<ToolSchema>,
    phase: Phase,

    /// Tool calls from the current assistant response, waiting to be executed.
    pending_tools: VecDeque<ToolCall>,
    /// The tool call currently being executed (yielded to caller).
    current_tool: Option<ToolCall>,

    /// Events queued for emission (FIFO). Drained before advancing the FSM.
    event_queue: VecDeque<Event>,

    /// Steering messages — injected by caller at any time. Checked before
    /// each tool execution and after each LLM response.
    steering_queue: VecDeque<Message>,

    /// Follow-up messages — only consumed when the loop would otherwise stop.
    follow_up_queue: VecDeque<Message>,

    /// Set by caller to abort the entire loop.
    aborted: bool,

    /// Consecutive transport errors (for retry logic).
    consecutive_transport_errors: u32,
    /// Max transport errors before giving up (0 = unlimited).
    max_transport_errors: u32,
}

impl AgentLoop {
    // ── Construction ─────────────────────────────────────────────────

    pub fn new(state: AgentState, tool_schemas: Vec<ToolSchema>) -> Self {
        Self {
            state,
            tool_schemas,
            phase: Phase::Init,
            pending_tools: VecDeque::new(),
            current_tool: None,
            event_queue: VecDeque::new(),
            steering_queue: VecDeque::new(),
            follow_up_queue: VecDeque::new(),
            aborted: false,
            consecutive_transport_errors: 0,
            max_transport_errors: 3,
        }
    }

    pub fn with_max_transport_errors(mut self, n: u32) -> Self {
        self.max_transport_errors = n;
        self
    }

    // ── Side-channel injection (callable at ANY time) ────────────────

    /// Inject a steering message. Takes effect at the next interaction point:
    /// remaining queued tool calls are skipped, and the message is appended
    /// to the conversation before the next LLM call.
    pub fn steer(&mut self, msg: Message) {
        self.steering_queue.push_back(msg);
    }

    /// Inject a follow-up message. Only consumed when the loop would
    /// otherwise finish (no tool calls, no steering).
    pub fn follow_up(&mut self, msg: Message) {
        self.follow_up_queue.push_back(msg);
    }

    /// Abort the entire loop. The next `resume_*` call will yield `Done(Aborted)`.
    pub fn abort(&mut self) {
        self.aborted = true;
    }

    // ── Read-only access to state ────────────────────────────────────

    pub fn state(&self) -> &AgentState {
        &self.state
    }

    pub fn is_done(&self) -> bool {
        matches!(self.phase, Phase::Done)
    }

    // ── The driver protocol ──────────────────────────────────────────

    /// Start the loop with initial user messages. Returns the first effect.
    pub fn start(&mut self, messages: Vec<Message>) -> Effect {
        self.state.messages.extend(messages);
        self.state.turn = 0;
        self.event_queue.push_back(Event::AgentStart);
        self.begin_turn()
    }

    /// Resume after an `Effect::NeedLlm`. Provide the LLM response or error.
    pub fn resume_llm(&mut self, result: Result<LlmResponse, TransportError>) -> Effect {
        assert!(matches!(self.phase, Phase::WaitingForLlm));

        if self.aborted {
            return self.finish(EndReason::Aborted);
        }

        match result {
            Err(e) => {
                self.consecutive_transport_errors += 1;
                if self.max_transport_errors > 0
                    && self.consecutive_transport_errors >= self.max_transport_errors
                {
                    return self.finish(EndReason::Error(format!(
                        "transport failed {} times, last: {e}",
                        self.consecutive_transport_errors
                    )));
                }
                // Retry: yield another NeedLlm with the same state.
                self.emit_need_llm()
            }
            Ok(resp) => {
                self.consecutive_transport_errors = 0;

                // Track tokens.
                self.state.total_input_tokens += resp.input_tokens;
                self.state.total_output_tokens += resp.output_tokens;
                self.event_queue.push_back(Event::TokenUsage {
                    input: resp.input_tokens,
                    output: resp.output_tokens,
                });

                // Record assistant message.
                self.event_queue.push_back(Event::AssistantMessage {
                    message: resp.message.clone(),
                });
                let tool_calls: Vec<ToolCall> = resp.message.tool_calls().cloned().collect();
                self.state.messages.push(resp.message);

                if resp.stop_reason == StopReason::Error {
                    return self.end_turn_and_maybe_continue();
                }

                if tool_calls.is_empty() {
                    // Model is done for this turn — no tools to run.
                    self.end_turn_and_maybe_continue()
                } else {
                    // Queue up tool calls.
                    self.pending_tools = tool_calls.into();
                    self.advance_to_next_tool()
                }
            }
        }
    }

    /// Resume after an `Effect::NeedToolExec`. Provide the execution result.
    pub fn resume_tool(&mut self, result: ToolExecResult) -> Effect {
        assert!(matches!(self.phase, Phase::WaitingForTool));

        let tc = self.current_tool.take().expect("no current tool call");

        if self.aborted {
            // Still record the result, then bail.
            self.push_tool_result(&tc, result);
            self.skip_remaining_tools();
            return self.finish(EndReason::Aborted);
        }

        // Record the result.
        let tool_result = ToolResult {
            tool_call_id: tc.id.clone(),
            content: result.content,
            is_error: result.is_error,
        };
        self.event_queue.push_back(Event::ToolExecEnd {
            call_id: tc.id.clone(),
            result: tool_result.clone(),
        });
        self.state.messages.push(Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult(tool_result)],
        });

        // Advance to next tool (or next LLM call).
        self.advance_to_next_tool()
    }

    /// Resume after an `Effect::Event`. No data needed — just acknowledgement
    /// that the caller processed the event (updated UI, etc.).
    pub fn resume(&mut self) -> Effect {
        assert!(matches!(self.phase, Phase::WaitingForEventAck));
        self.drain_or_advance()
    }

    // ── Internal state machine logic ─────────────────────────────────

    /// Drain queued events first; if none, advance the FSM.
    fn drain_or_advance(&mut self) -> Effect {
        // Always drain events before yielding an I/O effect.
        if let Some(event) = self.event_queue.pop_front() {
            self.phase = Phase::WaitingForEventAck;
            return Effect::Event(event);
        }

        // No events — advance based on current phase.
        match &self.phase {
            Phase::Init => self.emit_need_llm(),
            Phase::WaitingForEventAck => {
                // We were draining events. Now figure out what to actually do.
                // This shouldn't happen if drain logic is correct, but handle
                // it gracefully by emitting NeedLlm.
                self.emit_need_llm()
            }
            Phase::Done => Effect::Done(EndReason::Done),
            _ => unreachable!("drain_or_advance called in unexpected phase"),
        }
    }

    fn begin_turn(&mut self) -> Effect {
        self.state.turn += 1;
        self.event_queue
            .push_back(Event::TurnStart { turn: self.state.turn });
        self.phase = Phase::Init;
        self.drain_or_advance()
    }

    fn emit_need_llm(&mut self) -> Effect {
        // First drain any pending events.
        if let Some(event) = self.event_queue.pop_front() {
            self.phase = Phase::WaitingForEventAck;
            return Effect::Event(event);
        }

        self.phase = Phase::WaitingForLlm;
        Effect::NeedLlm(LlmRequest {
            system_prompt: self.state.system_prompt.clone(),
            messages: self.state.messages.clone(),
            tools: self.tool_schemas.clone(),
        })
    }

    fn advance_to_next_tool(&mut self) -> Effect {
        // Drain events first (e.g. ToolExecEnd from previous tool).
        if let Some(event) = self.event_queue.pop_front() {
            self.phase = Phase::WaitingForEventAck;
            return Effect::Event(event);
        }

        // Check for abort.
        if self.aborted {
            self.skip_remaining_tools();
            return self.finish(EndReason::Aborted);
        }

        // Check for steering — skip remaining tools if steered.
        if let Some(steer_msg) = self.steering_queue.pop_front() {
            self.event_queue.push_back(Event::Steered {
                message: steer_msg.clone(),
            });
            self.state.messages.push(steer_msg);
            self.skip_remaining_tools();
            // After steering, go back to the LLM with the new context.
            return self.drain_or_advance_to_llm();
        }

        // Pop next tool call.
        match self.pending_tools.pop_front() {
            Some(tc) => {
                self.event_queue.push_back(Event::ToolExecStart {
                    call_id: tc.id.clone(),
                    name: tc.name.clone(),
                });
                self.current_tool = Some(tc.clone());

                // Drain the ToolExecStart event first.
                if let Some(event) = self.event_queue.pop_front() {
                    self.phase = Phase::WaitingForEventAck;
                    return Effect::Event(event);
                }

                self.phase = Phase::WaitingForTool;
                Effect::NeedToolExec(ToolCallRequest {
                    call_id: tc.id,
                    name: tc.name,
                    arguments: tc.arguments,
                })
            }
            None => {
                // All tools done — loop back to LLM for the next turn.
                self.emit_need_llm()
            }
        }
    }

    /// After draining events (like Steered), proceed to the LLM call.
    fn drain_or_advance_to_llm(&mut self) -> Effect {
        if let Some(event) = self.event_queue.pop_front() {
            self.phase = Phase::WaitingForEventAck;
            return Effect::Event(event);
        }
        self.emit_need_llm()
    }

    fn end_turn_and_maybe_continue(&mut self) -> Effect {
        self.event_queue
            .push_back(Event::TurnEnd { turn: self.state.turn });

        // Check follow-up / steering before declaring done.
        if let Some(steer_msg) = self.steering_queue.pop_front() {
            self.state.messages.push(steer_msg);
            return self.begin_turn_after_events();
        }

        if let Some(follow_up) = self.follow_up_queue.pop_front() {
            self.state.messages.push(follow_up);
            return self.begin_turn_after_events();
        }

        // Truly done.
        self.finish(EndReason::Done)
    }

    fn begin_turn_after_events(&mut self) -> Effect {
        // Drain pending events (like TurnEnd), then start new turn.
        if let Some(event) = self.event_queue.pop_front() {
            // After draining, we need to remember to begin a new turn.
            // We use Init phase + pending events to handle this.
            self.phase = Phase::WaitingForEventAck;
            return Effect::Event(event);
        }
        self.begin_turn()
    }

    fn finish(&mut self, reason: EndReason) -> Effect {
        self.phase = Phase::Done;
        Effect::Done(reason)
    }

    fn skip_remaining_tools(&mut self) {
        while let Some(tc) = self.pending_tools.pop_front() {
            self.state.messages.push(Message::tool_result(
                &tc.id,
                vec![ContentBlock::Text("Skipped (interrupted)".into())],
                true,
            ));
        }
    }

    fn push_tool_result(&mut self, tc: &ToolCall, result: ToolExecResult) {
        let tool_result = ToolResult {
            tool_call_id: tc.id.clone(),
            content: result.content,
            is_error: result.is_error,
        };
        self.state.messages.push(Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult(tool_result)],
        });
    }
}

// ═══════════════════════════════════════════════════════════════════════
// PART 6: Convenience driver — `run_blocking`
// ═══════════════════════════════════════════════════════════════════════
//
// For callers who don't need fine-grained control, this function drives
// the loop synchronously, like v1. It's 20 lines on top of the FSM.

/// Trait for tools used with `run_blocking`.
pub trait Tool: Send {
    fn schema(&self) -> ToolSchema;
    fn execute(&self, call_id: &str, arguments: &str) -> ToolExecResult;
}

/// Simple blocking driver. Handles I/O, dispatches tools, logs events.
pub fn run_blocking(
    messages: Vec<Message>,
    state: AgentState,
    tools: &[&dyn Tool],
    transport: &dyn Fn(&LlmRequest) -> Result<LlmResponse, TransportError>,
    on_event: &mut dyn FnMut(&Event),
) -> (AgentState, EndReason) {
    let schemas = tools.iter().map(|t| t.schema()).collect::<Vec<_>>();
    let tool_map: std::collections::HashMap<&str, &dyn Tool> = tools
        .iter()
        .map(|t| (t.schema().name.as_str(), *t))
        .collect();

    let mut agent = AgentLoop::new(state, schemas);
    let mut effect = agent.start(messages);

    loop {
        match effect {
            Effect::Event(ref ev) => {
                on_event(ev);
                effect = agent.resume();
            }
            Effect::NeedLlm(ref req) => {
                let result = transport(req);
                effect = agent.resume_llm(result);
            }
            Effect::NeedToolExec(ref req) => {
                let result = match tool_map.get(req.name.as_str()) {
                    Some(tool) => tool.execute(&req.call_id, &req.arguments),
                    None => ToolExecResult::error(format!("unknown tool: {}", req.name)),
                };
                effect = agent.resume_tool(result);
            }
            Effect::Done(reason) => {
                // Move state out of the loop.
                return (agent.state.clone(), reason);
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// PART 7: Example — TUI driver with non-blocking tool execution
// ═══════════════════════════════════════════════════════════════════════
//
// This shows the *power* of the cooperative design: the caller can run
// tools in background threads, pump the UI at 60fps, and handle user
// interrupts — all without async.

#[cfg(never)]
mod tui_example {
    use super::*;
    use std::time::Duration;

    fn run_tui_agent(
        messages: Vec<Message>,
        state: AgentState,
        tools: &[&dyn Tool],
        transport: &dyn Fn(&LlmRequest) -> Result<LlmResponse, TransportError>,
        ui: &mut dyn TuiBackend,
    ) -> (AgentState, EndReason) {
        let schemas = tools.iter().map(|t| t.schema()).collect::<Vec<_>>();
        let tool_map: std::collections::HashMap<String, &dyn Tool> = tools
            .iter()
            .map(|t| (t.schema().name.clone(), *t))
            .collect();

        let mut agent = AgentLoop::new(state, schemas);
        let mut effect = agent.start(messages);

        loop {
            match effect {
                Effect::Event(ref ev) => {
                    ui.handle_event(ev);
                    ui.render();
                    effect = agent.resume();
                }

                Effect::NeedLlm(ref req) => {
                    // Start LLM call in background thread.
                    let req_clone = req.clone();
                    let (tx, rx) = std::sync::mpsc::channel();
                    std::thread::spawn(move || {
                        let result = todo!("call LLM");
                        let _ = tx.send(result);
                    });

                    // While waiting: pump UI, check for user input.
                    let result = loop {
                        // Check if LLM responded.
                        if let Ok(r) = rx.try_recv() {
                            break r;
                        }

                        // Poll user input — they might type a steering message.
                        if let Some(input) = ui.poll_input(Duration::from_millis(16)) {
                            match input {
                                UserInput::Abort => {
                                    agent.abort();
                                    // Still need to wait for the LLM call to finish
                                    // (or the transport to notice the abort).
                                    break rx.recv().unwrap();
                                }
                                UserInput::Message(text) => {
                                    agent.steer(Message::user(text));
                                }
                            }
                        }

                        // Render at ~60fps.
                        ui.render();
                    };

                    effect = agent.resume_llm(result);
                }

                Effect::NeedToolExec(ref req) => {
                    let tool = tool_map[req.name.as_str()];
                    let call_id = req.call_id.clone();
                    let args = req.arguments.clone();

                    // Run tool in background.
                    let (tx, rx) = std::sync::mpsc::channel();
                    std::thread::spawn(move || {
                        let result = tool.execute(&call_id, &args);
                        let _ = tx.send(result);
                    });

                    // While waiting: pump UI, check for interrupts.
                    let result = loop {
                        if let Ok(r) = rx.try_recv() {
                            break r;
                        }
                        if let Some(UserInput::Abort) = ui.poll_input(Duration::from_millis(16)) {
                            agent.abort();
                            break rx.recv().unwrap();
                        }
                        ui.render();
                    };

                    effect = agent.resume_tool(result);
                }

                Effect::Done(reason) => {
                    return (agent.state.clone(), reason);
                }
            }
        }
    }

    // Placeholder traits for the TUI example.
    enum UserInput {
        Abort,
        Message(String),
    }

    trait TuiBackend {
        fn handle_event(&mut self, event: &Event);
        fn render(&mut self);
        fn poll_input(&mut self, timeout: Duration) -> Option<UserInput>;
    }
}

// ═══════════════════════════════════════════════════════════════════════
// PART 8: Example — async driver (bridge to tokio)
// ═══════════════════════════════════════════════════════════════════════
//
// The core loop is sync, but integrating with async is trivial: spawn
// the I/O as async tasks and await them in the driver.

#[cfg(never)]
mod async_example {
    use super::*;

    async fn run_async_agent(
        messages: Vec<Message>,
        state: AgentState,
        schemas: Vec<ToolSchema>,
    ) -> (AgentState, EndReason) {
        let mut agent = AgentLoop::new(state, schemas);
        let mut effect = agent.start(messages);

        loop {
            match effect {
                Effect::Event(ref _ev) => {
                    // Handle event (log, websocket push, etc.)
                    effect = agent.resume();
                }
                Effect::NeedLlm(ref req) => {
                    // Use an async HTTP client.
                    let result = todo!("async_llm_call(req).await");
                    effect = agent.resume_llm(result);
                }
                Effect::NeedToolExec(ref req) => {
                    // Use tokio::task::spawn_blocking for CPU-bound tools,
                    // or just await async tools directly.
                    let result = todo!("async_tool_exec(req).await");
                    effect = agent.resume_tool(result);
                }
                Effect::Done(reason) => {
                    return (agent.state.clone(), reason);
                }
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// PART 9: Extension semantics
// ═══════════════════════════════════════════════════════════════════════
//
// The v2 design has *fewer* built-in extension points than v1 because the
// cooperative model pushes most concerns to the driver:
//
// ┌─────────────────────────────────────────────────────────────────────┐
// │ Concern             │ v1 (blocking)       │ v2 (cooperative)       │
// ├─────────────────────┼─────────────────────┼────────────────────────┤
// │ Tool dispatch       │ Tool trait objects   │ Caller's problem.      │
// │                     │ inside the loop      │ Loop yields requests.  │
// ├─────────────────────┼─────────────────────┼────────────────────────┤
// │ Transport           │ Transport trait      │ Caller's problem.      │
// │                     │ inside the loop      │ Loop yields requests.  │
// ├─────────────────────┼─────────────────────┼────────────────────────┤
// │ Event handling      │ EventSink trait      │ Caller handles Effect  │
// │                     │                     │ ::Event directly.      │
// ├─────────────────────┼─────────────────────┼────────────────────────┤
// │ Context transform   │ ContextTransform    │ Caller mutates the     │
// │                     │ trait               │ LlmRequest before      │
// │                     │                     │ sending. No trait.     │
// ├─────────────────────┼─────────────────────┼────────────────────────┤
// │ Streaming tokens    │ Not possible (loop  │ Caller streams while   │
// │                     │ blocks in send())   │ holding NeedLlm.       │
// ├─────────────────────┼─────────────────────┼────────────────────────┤
// │ UI updates          │ Via EventSink only   │ At every yield point.  │
// ├─────────────────────┼─────────────────────┼────────────────────────┤
// │ Interrupts          │ AbortFlag + steering │ steer()/abort() at     │
// │                     │ (racy)              │ any yield point (safe). │
// ├─────────────────────┼─────────────────────┼────────────────────────┤
// │ Parallel tools      │ Not supported        │ Caller spawns threads  │
// │                     │                     │ while holding NeedTool.│
// ├─────────────────────┼─────────────────────┼────────────────────────┤
// │ Retry/backoff       │ In Transport impl    │ Caller retries before  │
// │                     │                     │ calling resume_llm().  │
// ├─────────────────────┼─────────────────────┼────────────────────────┤
// │ Session branching   │ Clone AgentState     │ Clone AgentState.      │
// │                     │                     │ (same as v1)           │
// └─────────────────────┴─────────────────────┴────────────────────────┘
//
// Key insight: the cooperative design REMOVES built-in extension traits
// (Transport, EventSink, ContextTransform) and replaces them with
// interaction points. This is *more* extensible because:
//
//   1. No trait object overhead or dynamic dispatch.
//   2. Callers can compose behaviors freely (retry + cache + log)
//      without wrapper-trait towers.
//   3. The core has zero dependencies beyond std.
//   4. Testing is trivial — feed canned Effects, assert on Resumes.
//
// The only extension surface the core provides is:
//   - Tool schemas (what capabilities to advertise to the model)
//   - steer() / follow_up() / abort() (injecting user intent)
//
// Everything else is the driver's responsibility.

// ═══════════════════════════════════════════════════════════════════════
// PART 10: Design notes
// ═══════════════════════════════════════════════════════════════════════
//
// THE EFFECT PATTERN (aka "algebraic effects", "free monad", "command pattern")
//
// The core idea: separate *describing* what to do from *doing* it.
// The loop is a pure function: (State, Input) → (State, Effect).
// The driver is the interpreter that gives meaning to effects.
//
// This pattern is well-established in:
//   - Elm's Cmd/Sub model
//   - Redux-saga's effects
//   - Haskell's free monads
//   - Game engines' command buffers
//
// In Rust, it maps naturally to enums + match, with no special language
// support needed. No Pin, no Future, no async, no trait objects in the
// core — just enums and structs.
//
//
// WHY EVENTS ARE IN THE EFFECT ENUM (not a separate channel):
//
// Events are yielded as `Effect::Event` rather than collected in a Vec
// because:
//   1. The caller gets to process each event *before* the loop advances.
//      This guarantees UI consistency — you never miss a state transition.
//   2. Backpressure is natural: the loop won't flood the caller with
//      events faster than it can handle them.
//   3. No interior mutability or channels needed.
//   4. The caller can trivially skip events: just call resume() immediately.
//
// If event-per-yield is too chatty, callers can batch:
//
//   loop {
//       match effect {
//           Effect::Event(ev) => {
//               events.push(ev);
//               effect = agent.resume();  // immediately get next
//               continue;                 // batch until non-event
//           }
//           _ => {
//               process_batched_events(&events);
//               events.clear();
//               // handle the non-event effect...
//           }
//       }
//   }
//
//
// COMPARISON WITH ASYNC:
//
// An async version of this same design would use `async fn` + `.await`
// instead of yield/resume:
//
//   async fn agent_loop(state, tools, transport) {
//       loop {
//           let resp = transport.send(&request).await;  // <-- yield point
//           for tc in resp.tool_calls() {
//               let result = tools.execute(tc).await;   // <-- yield point
//           }
//       }
//   }
//
// The cooperative design achieves the same yield points without async.
// Advantages over async:
//   - No executor dependency (works in no_std, embedded, WASM, etc.)
//   - No Send/Sync/Pin constraints propagating through the entire stack
//   - Easier to test (no need to mock async boundaries)
//   - State is always accessible (no borrow-across-await issues)
//   - The driver can be async OR sync — caller's choice
//
// Disadvantage:
//   - More boilerplate in the driver (the match loop).
//     Mitigated by run_blocking() convenience function.

fn main() {
    println!("See the code and comments above for the proposed architecture.");
}
