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
// PART 9: Hook layer — composable extension points
// ═══════════════════════════════════════════════════════════════════════
//
// The pure effect/resume model gives the driver full control, but
// "everything is the driver's problem" doesn't scale when you have 5+
// independent extensions (approval gates, caching, metrics, custom tools,
// context compaction). Each would need to be hand-wired into the driver's
// match loop, and they'd have no way to compose without knowing about
// each other.
//
// The solution: a **Hook trait** that sits between the AgentLoop and the
// driver's I/O. Hooks are middleware — they intercept effects and resumes
// at well-defined points, can modify data flowing through, and compose
// in an ordered pipeline.
//
// ┌────────────┐  Effect  ┌───────────────┐  Effect  ┌────────────┐
// │ AgentLoop  │ ───────> │ HookPipeline  │ ───────> │   Driver   │
// │  (pure)    │ <─────── │ (middleware)   │ <─────── │   (I/O)    │
// │            │  Resume  │               │  Resume  │            │
// └────────────┘          └───────────────┘          └────────────┘
//
// This maps directly to pi-mono's extension model:
//
// ┌────────────────────────┬─────────────────────────────────────────┐
// │ pi-mono event          │ Hook method                            │
// ├────────────────────────┼─────────────────────────────────────────┤
// │ pi.on("context")       │ Hook::before_llm(&mut LlmRequest)      │
// │ pi.on("tool_call")     │ Hook::before_tool() → ToolAction       │
// │ pi.on("tool_result")   │ Hook::after_tool(&mut ToolExecResult)   │
// │ pi.on("agent_start")   │ Hook::on_start()                       │
// │ pi.on("agent_end")     │ Hook::on_end()                         │
// │ pi.on("turn_start")    │ Hook::on_event(TurnStart)              │
// │ pi.on("turn_end")      │ Hook::on_event(TurnEnd)                │
// │ pi.registerTool()      │ Hook::tools() → Vec<ToolSchema>        │
// │ pi.registerTool.exec() │ Hook::execute_tool() → Option<Result>  │
// └────────────────────────┴─────────────────────────────────────────┘
//
// Key design constraints:
//   1. The AgentLoop knows NOTHING about hooks. Zero coupling.
//   2. Hooks are optional — plain drivers work without them.
//   3. Hooks compose in order: first hook to block/replace wins.
//   4. No trait objects in the core crate. Hook lives in the driver crate.
//   5. Hooks can provide tools (like pi.registerTool) AND execute them.

/// What a hook wants to do with a tool call.
pub enum ToolAction {
    /// Let it through — next hook (or the driver) handles it.
    Continue,
    /// Block execution entirely. The tool result will be an error
    /// with the given reason.
    Block { reason: String },
    /// Short-circuit with this result. The actual tool never runs.
    /// Useful for caching, mocking, sandboxing.
    Replace(ToolExecResult),
}

/// Read-only view of agent state available to hooks.
pub struct HookContext<'a> {
    pub state: &'a AgentState,
    pub turn: usize,
    pub aborted: bool,
}

/// The extension point. Each method has a default no-op implementation,
/// so hooks only override what they care about.
///
/// Hooks run in pipeline order. For `before_tool`, the first hook to
/// return `Block` or `Replace` wins (short-circuit). For mutation methods
/// (`before_llm`, `after_tool`, `after_llm`), all hooks run in sequence,
/// each seeing the mutations from previous hooks.
pub trait Hook {
    /// Human-readable name for debugging / logging.
    fn name(&self) -> &str { "unnamed" }

    // ── Lifecycle ────────────────────────────────────────────────────

    /// Called once when the agent loop starts.
    fn on_start(&mut self, _ctx: &HookContext) {}

    /// Called once when the agent loop finishes.
    fn on_end(&mut self, _reason: &EndReason, _ctx: &HookContext) {}

    /// Called for every Event the loop emits (turn boundaries, token
    /// deltas, etc.). Hooks can log, update metrics, or push to UI.
    fn on_event(&mut self, _event: &Event, _ctx: &HookContext) {}

    // ── LLM interception ────────────────────────────────────────────

    /// Called before each LLM request is sent. Can mutate the request
    /// (add system messages, inject context, change model params).
    /// This is pi-mono's `context` event.
    fn before_llm(&mut self, _req: &mut LlmRequest, _ctx: &HookContext) {}

    /// Called after the LLM response arrives. Can inspect or mutate.
    fn after_llm(
        &mut self,
        _req: &LlmRequest,
        _resp: &mut Result<LlmResponse, TransportError>,
        _ctx: &HookContext,
    ) {}

    // ── Tool interception ───────────────────────────────────────────

    /// Called before a tool executes. Can:
    /// - `Continue`: pass through to the next hook / actual execution
    /// - `Block { reason }`: reject the call (becomes an error result)
    /// - `Replace(result)`: skip execution, use this result instead
    ///
    /// The request is mutable — hooks can rewrite arguments.
    /// This is pi-mono's `tool_call` event (with `{ block: true }`).
    fn before_tool(
        &mut self,
        _req: &mut ToolCallRequest,
        _ctx: &HookContext,
    ) -> ToolAction {
        ToolAction::Continue
    }

    /// Called after a tool executes (or after a Replace). Can mutate
    /// the result. This is pi-mono's `tool_result` event.
    fn after_tool(
        &mut self,
        _req: &ToolCallRequest,
        _result: &mut ToolExecResult,
        _ctx: &HookContext,
    ) {}

    // ── Tool provision ──────────────────────────────────────────────

    /// Return additional tool schemas this hook provides.
    /// These get merged with the base schemas when building LlmRequests.
    /// This is pi-mono's `pi.registerTool()`.
    fn tools(&self) -> Vec<ToolSchema> { vec![] }

    /// Execute a tool that this hook provides. Called only for tools
    /// whose names match schemas returned by `tools()`. Return `None`
    /// to fall through to the next hook or the driver's tool map.
    fn execute_tool(
        &mut self,
        _call_id: &str,
        _name: &str,
        _arguments: &str,
        _ctx: &HookContext,
    ) -> Option<ToolExecResult> {
        None
    }
}

/// Ordered list of hooks. Runs them in sequence for each interception point.
pub struct HookPipeline {
    hooks: Vec<Box<dyn Hook>>,
}

impl HookPipeline {
    pub fn new() -> Self {
        HookPipeline { hooks: Vec::new() }
    }

    /// Add a hook to the end of the pipeline. Order matters:
    /// hooks added first run first and can short-circuit later ones.
    pub fn add(&mut self, hook: impl Hook + 'static) {
        self.hooks.push(Box::new(hook));
    }

    pub fn on_start(&mut self, ctx: &HookContext) {
        for h in &mut self.hooks { h.on_start(ctx); }
    }

    pub fn on_end(&mut self, reason: &EndReason, ctx: &HookContext) {
        for h in &mut self.hooks { h.on_end(reason, ctx); }
    }

    pub fn on_event(&mut self, event: &Event, ctx: &HookContext) {
        for h in &mut self.hooks { h.on_event(event, ctx); }
    }

    pub fn before_llm(&mut self, req: &mut LlmRequest, ctx: &HookContext) {
        for h in &mut self.hooks { h.before_llm(req, ctx); }
    }

    pub fn after_llm(
        &mut self,
        req: &LlmRequest,
        resp: &mut Result<LlmResponse, TransportError>,
        ctx: &HookContext,
    ) {
        for h in &mut self.hooks { h.after_llm(req, resp, ctx); }
    }

    /// Run before_tool hooks. First hook to return Block or Replace wins.
    pub fn before_tool(
        &mut self,
        req: &mut ToolCallRequest,
        ctx: &HookContext,
    ) -> ToolAction {
        for h in &mut self.hooks {
            match h.before_tool(req, ctx) {
                ToolAction::Continue => continue,
                action => return action,
            }
        }
        ToolAction::Continue
    }

    pub fn after_tool(
        &mut self,
        req: &ToolCallRequest,
        result: &mut ToolExecResult,
        ctx: &HookContext,
    ) {
        for h in &mut self.hooks { h.after_tool(req, result, ctx); }
    }

    /// Collect all tool schemas from all hooks.
    pub fn tool_schemas(&self) -> Vec<ToolSchema> {
        self.hooks.iter().flat_map(|h| h.tools()).collect()
    }

    /// Try to execute a tool via hooks. Returns None if no hook handles it.
    pub fn execute_tool(
        &mut self,
        call_id: &str,
        name: &str,
        arguments: &str,
        ctx: &HookContext,
    ) -> Option<ToolExecResult> {
        for h in &mut self.hooks {
            if let Some(result) = h.execute_tool(call_id, name, arguments, ctx) {
                return Some(result);
            }
        }
        None
    }
}

// ═══════════════════════════════════════════════════════════════════════
// PART 10: run_with_hooks — the standard driver with hook integration
// ═══════════════════════════════════════════════════════════════════════
//
// This replaces `run_blocking` for callers who use hooks. The match loop
// is the same, but every transition flows through the HookPipeline.
// Callers who don't need hooks can still use `run_blocking`.

pub fn run_with_hooks(
    messages: Vec<Message>,
    state: AgentState,
    tools: &[&dyn Tool],
    transport: &dyn Fn(&LlmRequest) -> Result<LlmResponse, TransportError>,
    hooks: &mut HookPipeline,
    on_event: &mut dyn FnMut(&Event),
) -> (AgentState, EndReason) {
    // Merge base tool schemas with hook-provided schemas.
    let mut schemas: Vec<ToolSchema> = tools.iter().map(|t| t.schema()).collect();
    schemas.extend(hooks.tool_schemas());

    let tool_map: std::collections::HashMap<&str, &dyn Tool> = tools
        .iter()
        .map(|t| (t.schema().name.as_str(), *t))
        .collect();

    let mut agent = AgentLoop::new(state, schemas);
    let mut effect = agent.start(messages);
    let mut turn: usize = 0;

    let ctx = HookContext { state: &agent.state, turn, aborted: false };
    hooks.on_start(&ctx);

    loop {
        let ctx = HookContext { state: &agent.state, turn, aborted: false };

        match effect {
            Effect::Event(ref ev) => {
                // Let hooks observe events (metrics, logging, UI).
                hooks.on_event(ev, &ctx);
                on_event(ev);

                // Track turn boundaries.
                if matches!(ev, Event::TurnStart) { turn += 1; }

                effect = agent.resume();
            }

            Effect::NeedLlm(ref mut req) => {
                // Hook: context transform, prompt injection, model override.
                hooks.before_llm(req, &ctx);

                let mut result = transport(req);

                // Hook: inspect/modify response, log, cache.
                hooks.after_llm(req, &mut result, &ctx);

                effect = agent.resume_llm(result);
            }

            Effect::NeedToolExec(ref mut req) => {
                // Hook: approval gate, argument rewriting, blocking.
                let action = hooks.before_tool(req, &ctx);

                let mut result = match action {
                    ToolAction::Continue => {
                        // Try hook-provided tools first, then the base tool map.
                        hooks.execute_tool(&req.call_id, &req.name, &req.arguments, &ctx)
                            .unwrap_or_else(|| {
                                match tool_map.get(req.name.as_str()) {
                                    Some(tool) => tool.execute(&req.call_id, &req.arguments),
                                    None => ToolExecResult::error(
                                        format!("unknown tool: {}", req.name)
                                    ),
                                }
                            })
                    }
                    ToolAction::Block { reason } => {
                        ToolExecResult::error(format!("blocked by hook: {}", reason))
                    }
                    ToolAction::Replace(result) => result,
                };

                // Hook: modify result, add metadata, sanitize output.
                hooks.after_tool(req, &mut result, &ctx);

                effect = agent.resume_tool(result);
            }

            Effect::Done(ref reason) => {
                let ctx = HookContext { state: &agent.state, turn, aborted: false };
                hooks.on_end(reason, &ctx);
                return (agent.state.clone(), reason.clone());
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// PART 11: Concrete hook examples
// ═══════════════════════════════════════════════════════════════════════
//
// These show how pi-mono-style extensions map onto the Hook trait.
// Each is self-contained — they compose by adding them to a pipeline.

#[cfg(never)]
mod hook_examples {
    use super::*;
    use std::collections::HashMap;

    // ─── Example 1: Approval Gate ────────────────────────────────────
    //
    // Equivalent to pi-mono's:
    //   pi.on("tool_call", async (event, ctx) => {
    //     if (event.toolName === "bash" && event.input.command?.includes("rm")) {
    //       const ok = await ctx.ui.confirm("Allow?");
    //       if (!ok) return { block: true, reason: "User denied" };
    //     }
    //   });

    struct ApprovalGate {
        /// Tool names that require user approval.
        gated_tools: Vec<String>,
        /// Callback that asks the user. Returns true = allow.
        ask: Box<dyn FnMut(&ToolCallRequest) -> bool>,
    }

    impl Hook for ApprovalGate {
        fn name(&self) -> &str { "approval-gate" }

        fn before_tool(
            &mut self,
            req: &mut ToolCallRequest,
            _ctx: &HookContext,
        ) -> ToolAction {
            if self.gated_tools.iter().any(|t| t == &req.name) {
                if (self.ask)(req) {
                    ToolAction::Continue
                } else {
                    ToolAction::Block {
                        reason: format!("User denied execution of '{}'", req.name),
                    }
                }
            } else {
                ToolAction::Continue
            }
        }
    }

    // ─── Example 2: Response Cache ───────────────────────────────────
    //
    // Caches LLM responses by request hash. Useful for deterministic
    // replay, testing, and cost reduction during development.

    struct ResponseCache {
        cache: HashMap<u64, LlmResponse>,
    }

    impl ResponseCache {
        fn hash_request(req: &LlmRequest) -> u64 {
            // In practice, use a proper hasher over serialized request.
            0 // placeholder
        }
    }

    impl Hook for ResponseCache {
        fn name(&self) -> &str { "response-cache" }

        fn before_llm(&mut self, req: &mut LlmRequest, _ctx: &HookContext) {
            // Could inject a cache-control header or marker.
        }

        fn after_llm(
            &mut self,
            req: &LlmRequest,
            resp: &mut Result<LlmResponse, TransportError>,
            _ctx: &HookContext,
        ) {
            let key = Self::hash_request(req);
            if let Ok(r) = resp {
                self.cache.insert(key, r.clone());
            }
        }

        // To serve from cache, you'd wrap the transport function itself.
        // Or add a before_llm that sets a flag checked by the driver.
        // The hook layer doesn't replace the transport — it wraps it.
    }

    // ─── Example 3: Metrics Collector ────────────────────────────────
    //
    // Counts turns, tool calls, tokens, errors. Pure observer — never
    // modifies anything.

    struct Metrics {
        turns: usize,
        tool_calls: usize,
        tool_errors: usize,
        llm_calls: usize,
        llm_errors: usize,
    }

    impl Hook for Metrics {
        fn name(&self) -> &str { "metrics" }

        fn on_event(&mut self, event: &Event, _ctx: &HookContext) {
            if matches!(event, Event::TurnStart) {
                self.turns += 1;
            }
        }

        fn after_llm(
            &mut self,
            _req: &LlmRequest,
            resp: &mut Result<LlmResponse, TransportError>,
            _ctx: &HookContext,
        ) {
            self.llm_calls += 1;
            if resp.is_err() { self.llm_errors += 1; }
        }

        fn after_tool(
            &mut self,
            _req: &ToolCallRequest,
            result: &mut ToolExecResult,
            _ctx: &HookContext,
        ) {
            self.tool_calls += 1;
            if result.is_error { self.tool_errors += 1; }
        }

        fn on_end(&mut self, _reason: &EndReason, _ctx: &HookContext) {
            eprintln!(
                "[metrics] turns={} tools={} (err={}) llm={} (err={})",
                self.turns, self.tool_calls, self.tool_errors,
                self.llm_calls, self.llm_errors,
            );
        }
    }

    // ─── Example 4: Extension-Provided Tool ──────────────────────────
    //
    // Equivalent to pi-mono's:
    //   pi.registerTool({
    //     name: "todo_list",
    //     parameters: Type.Object({ action: StringEnum(["add", "list"]) }),
    //     async execute(id, params, signal, onUpdate, ctx) { ... }
    //   });
    //
    // The hook provides both the schema (so the LLM knows about it)
    // and the execution (so the driver doesn't need a separate tool map
    // entry).

    struct TodoListHook {
        items: Vec<String>,
    }

    impl Hook for TodoListHook {
        fn name(&self) -> &str { "todo-list" }

        fn tools(&self) -> Vec<ToolSchema> {
            vec![ToolSchema {
                name: "todo_list".into(),
                description: "Manage a todo list".into(),
                parameters: r#"{
                    "type": "object",
                    "properties": {
                        "action": { "type": "string", "enum": ["add", "list", "clear"] },
                        "text": { "type": "string" }
                    },
                    "required": ["action"]
                }"#.into(),
            }]
        }

        fn execute_tool(
            &mut self,
            _call_id: &str,
            name: &str,
            arguments: &str,
            _ctx: &HookContext,
        ) -> Option<ToolExecResult> {
            if name != "todo_list" { return None; }

            // In practice, parse arguments properly.
            let result = if arguments.contains("\"list\"") {
                let list = self.items.iter()
                    .enumerate()
                    .map(|(i, s)| format!("{}. {}", i + 1, s))
                    .collect::<Vec<_>>()
                    .join("\n");
                ToolExecResult::success(if list.is_empty() {
                    "No items.".into()
                } else {
                    list
                })
            } else if arguments.contains("\"add\"") {
                // Extract text (simplified).
                self.items.push("new item".into());
                ToolExecResult::success("Added.".into())
            } else if arguments.contains("\"clear\"") {
                self.items.clear();
                ToolExecResult::success("Cleared.".into())
            } else {
                ToolExecResult::error("Unknown action".into())
            };

            Some(result)
        }
    }

    // ─── Example 5: Context Compaction ───────────────────────────────
    //
    // Equivalent to pi-mono's `context` event handler that trims the
    // conversation when it gets too long. Watches token counts and
    // rewrites the LlmRequest's messages before they're sent.

    struct ContextCompactor {
        max_tokens: usize,
    }

    impl Hook for ContextCompactor {
        fn name(&self) -> &str { "context-compactor" }

        fn before_llm(&mut self, req: &mut LlmRequest, ctx: &HookContext) {
            // Estimate token count (simplified).
            let estimated_tokens: usize = req.messages.iter()
                .flat_map(|m| &m.content)
                .map(|b| match b {
                    ContentBlock::Text(t) => t.len() / 4, // rough estimate
                    _ => 50,
                })
                .sum();

            if estimated_tokens > self.max_tokens {
                // Strategy: keep system + first message + last N messages.
                // In practice, you'd summarize the middle.
                let len = req.messages.len();
                if len > 4 {
                    let keep_start = 1; // system prompt
                    let keep_end = 3;   // recent context
                    let summary = Message::user(format!(
                        "[Context compacted: {} messages summarized]",
                        len - keep_start - keep_end,
                    ));
                    let mut compacted = Vec::new();
                    compacted.extend_from_slice(&req.messages[..keep_start]);
                    compacted.push(summary);
                    compacted.extend_from_slice(&req.messages[len - keep_end..]);
                    req.messages = compacted;
                }
            }
        }
    }

    // ─── Composition ─────────────────────────────────────────────────
    //
    // All five hooks compose in a single pipeline. Order matters:
    // - ApprovalGate runs before_tool FIRST (can block before anything else)
    // - Metrics runs after_tool LAST (sees the final result)

    fn example_composition() {
        let mut pipeline = HookPipeline::new();

        // Order: approval checks first, then tool provision, then metrics last.
        pipeline.add(ApprovalGate {
            gated_tools: vec!["bash".into(), "write".into()],
            ask: Box::new(|req| {
                eprintln!("Allow {}? [y/n]", req.name);
                true // placeholder
            }),
        });
        pipeline.add(TodoListHook { items: vec![] });
        pipeline.add(ContextCompactor { max_tokens: 100_000 });
        pipeline.add(ResponseCache { cache: HashMap::new() });
        pipeline.add(Metrics {
            turns: 0, tool_calls: 0, tool_errors: 0,
            llm_calls: 0, llm_errors: 0,
        });

        // Now pass `pipeline` to `run_with_hooks()`.
        // Each hook fires at its interception points, in pipeline order.
        // They don't know about each other — they just see the data
        // flowing through.
    }
}

// ═══════════════════════════════════════════════════════════════════════
// PART 12: Design notes
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
// WHY HOOKS ARE A SEPARATE LAYER (not baked into AgentLoop):
//
// Hooks live in the driver crate, not the core. This preserves:
//   1. Core purity — AgentLoop has zero knowledge of Hook.
//   2. Optional adoption — you can use AgentLoop without hooks.
//   3. Custom hook implementations — nothing stops you from writing
//      your own hook dispatch instead of using HookPipeline.
//   4. Testability — test hooks in isolation by calling their methods
//      directly with mock HookContexts.
//
// The Hook trait uses dynamic dispatch (Box<dyn Hook>) because:
//   - Extensions are loaded at runtime (from config, CLI flags, etc.)
//   - The number of hooks is small (5-20), so vtable overhead is noise
//   - It matches how pi-mono loads extensions (dynamic, hot-reloadable)
//   - For compile-time-known hooks, you can skip the trait entirely
//     and just call functions directly in your driver loop.
//
//
// WHY HOOKS USE RETURN VALUES (not callbacks):
//
// pi-mono's `{ block: true, reason: "..." }` return pattern maps cleanly
// to our `ToolAction` enum. This avoids the borrow-checker headaches of
// hooks mutating shared state through callbacks:
//
//   // BAD: callback-based (borrow hell in Rust)
//   hook.before_tool(req, |agent| { agent.abort(); });
//
//   // GOOD: return-value-based (the driver acts on the return)
//   match hook.before_tool(req) {
//       ToolAction::Block { reason } => { /* driver handles it */ }
//       ...
//   }
//
//
// COMPARISON WITH PI-MONO'S EXTENSION MODEL:
//
// pi-mono uses an event emitter (`pi.on("tool_call", handler)`) with
// async handlers that receive an ExtensionContext. Our Hook trait is
// the Rust equivalent:
//
//   - `pi.on("event_name", handler)` → implement the corresponding
//     Hook method.
//   - `pi.registerTool(schema)` → return it from `Hook::tools()`.
//   - `pi.registerCommand("/cmd")` → not in the core; the driver
//     registers commands separately (they're UI, not agent logic).
//   - `ctx.ui.confirm()` → the hook's `ask` callback (or channel)
//     bridges to whatever UI system the driver uses.
//   - `pi.sendMessage()` → call `agent.steer()` from the driver after
//     a hook returns an action that requires injecting a message.
//   - Hot-reload → replace hooks in the pipeline at runtime. The
//     `HookPipeline` owns `Box<dyn Hook>` so you can swap them.
//
//
// WHAT HOOKS CAN'T DO (by design):
//
//   - Hooks cannot directly mutate AgentLoop internal state.
//     They see a read-only HookContext. Mutations flow through the
//     effect/resume protocol (modify the LlmRequest, modify the
//     ToolExecResult, return ToolAction::Block, etc.).
//
//   - Hooks cannot inject arbitrary messages into the conversation.
//     That's the driver's job via agent.steer() / agent.follow_up().
//     A hook can *signal* to the driver that it should steer (e.g.,
//     by returning a custom ToolAction variant or setting a flag),
//     but the driver decides when and how.
//
//   - Hooks cannot change the flow of the state machine (skip a turn,
//     force a retry). The FSM's transitions are fixed. Hooks influence
//     the *data* flowing through transitions, not the transitions
//     themselves.
//
// This keeps the core loop's behavior predictable and auditable,
// while still allowing powerful extension points at every I/O boundary.

fn main() {
    println!("See the code and comments above for the proposed architecture.");
}
