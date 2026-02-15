// =============================================================================
// Pi Agent Core Loop — Rust Proposal (v3: Cooperative FSM + Hooks)
// =============================================================================
//
// The core loop is a **cooperative state machine** that never blocks. It yields
// `Effect` values at every I/O boundary — the caller (the "driver") performs
// the actual I/O and feeds results back.
//
// **Hooks** are extension points passed to every resume_*() call. The agent
// calls hooks internally — the driver just passes them through. This keeps
// the driver trivially simple (just I/O) while allowing powerful extension
// at every interaction point.
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
// ┌─────────────────────────────────────────────────────────────────────────┐
// │                           ARCHITECTURE                                  │
// │                                                                        │
// │   ┌───────────────────────────────────┐                                │
// │   │          AgentLoop (FSM)          │                                │
// │   │                                   │                                │
// │   │  On every resume_*() call:        │                                │
// │   │    1. Process input (LLM response, │       ┌──────────────┐        │
// │   │       tool result, event ack)     │       │              │        │
// │   │    2. Call hooks internally ───────┤──────>│  &mut Hooks  │        │
// │   │       (before_llm, before_tool,   │<──────│  (borrowed)  │        │
// │   │        after_tool, etc.)          │       │              │        │
// │   │    3. Handle hook results         │       └──────────────┘        │
// │   │       (Block → error result,      │        hooks own their state   │
// │   │        Replace → use cached, etc.)│                                │
// │   │    4. Yield next Effect           │                                │
// │   │                                   │                                │
// │   └─────────┬─────────────────────────┘                                │
// │      Effect │ ▲ Resume                                                 │
// │             ▼ │                                                        │
// │   ┌───────────────────────────────────┐                                │
// │   │     Driver (your code — just I/O) │                                │
// │   │                                   │                                │
// │   │  NeedLlm(req) → call transport    │                                │
// │   │  NeedToolExec(req) → run tool     │                                │
// │   │  Event(ev) → update UI/log        │                                │
// │   │  Done(reason) → break             │                                │
// │   └───────────────────────────────────┘                                │
// └─────────────────────────────────────────────────────────────────────────┘

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
// PART 4b: Hooks — extension points passed on every step
// ═══════════════════════════════════════════════════════════════════════
//
// Hooks are NOT stored by the AgentLoop. They're borrowed on every
// resume_*() call. The agent calls them internally at the right points
// and handles the results (blocked tools, replaced results) itself.
//
// The driver just passes hooks through — it doesn't need to know what
// they do. This keeps the driver trivially simple.
//
//   // Driver loop — hooks are opaque, just passed through:
//   loop {
//       match effect {
//           Effect::NeedLlm(req) => {
//               let result = transport(&req);
//               effect = agent.resume_llm(result, &mut hooks);
//           }
//           Effect::NeedToolExec(req) => {
//               let result = execute(&req);
//               effect = agent.resume_tool(result, &mut hooks);
//           }
//           ...
//       }
//   }
//
// Hooks own their state themselves. The agent doesn't care.

/// What a hook wants to do with a tool call.
#[derive(Debug)]
pub enum ToolAction {
    /// Let it through — execute normally.
    Continue,
    /// Block execution. The agent handles it internally: creates an
    /// error result, records it, moves to the next tool. The driver
    /// never sees this tool call.
    Block { reason: String },
    /// Short-circuit with this result. The actual tool never runs.
    /// The driver never sees this tool call.
    Replace(ToolExecResult),
}

/// The extension point. Passed to every `resume_*()` call. The agent
/// calls these methods internally at the right points.
///
/// All methods have default no-op implementations, so hooks only
/// override what they care about. Use `NoHooks` for zero overhead.
pub trait Hooks {
    /// Called before each LLM request is built and yielded.
    /// Can mutate the request (add system context, rewrite messages,
    /// change tool schemas, etc.). This is the "context transform" hook.
    fn before_llm(&mut self, _req: &mut LlmRequest, _state: &AgentState) {}

    /// Called after the LLM response arrives, before the agent processes it.
    /// Can inspect or mutate the response.
    fn after_llm(
        &mut self,
        _resp: &mut LlmResponse,
        _state: &AgentState,
    ) {}

    /// Called before each tool executes. Can:
    /// - `Continue`: execute normally (driver handles it)
    /// - `Block { reason }`: reject. Agent records error, moves on.
    /// - `Replace(result)`: skip execution, use this result instead.
    ///
    /// The request is mutable — hooks can rewrite arguments.
    /// If the hook returns Block or Replace, the driver never sees
    /// this tool call. The agent handles it internally.
    fn before_tool(
        &mut self,
        _req: &mut ToolCallRequest,
        _state: &AgentState,
    ) -> ToolAction {
        ToolAction::Continue
    }

    /// Called after a tool executes. Can mutate the result before
    /// the agent records it.
    fn after_tool(
        &mut self,
        _req: &ToolCallRequest,
        _result: &mut ToolExecResult,
        _state: &AgentState,
    ) {}

    /// Called for every event the agent emits (before yielding it).
    fn on_event(&mut self, _event: &Event, _state: &AgentState) {}
}

/// Zero-cost no-op hooks. All calls inline to nothing.
pub struct NoHooks;
impl Hooks for NoHooks {}

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
    //
    // Every method takes `&mut impl Hooks`. The agent calls hooks
    // internally at the right points. The driver just passes them
    // through without knowing what they do.
    //
    // With `NoHooks`, all hook calls are inlined to nothing by the
    // compiler — zero overhead.

    /// Start the loop with initial user messages. Returns the first effect.
    pub fn start(&mut self, messages: Vec<Message>, hooks: &mut impl Hooks) -> Effect {
        self.state.messages.extend(messages);
        self.state.turn = 0;
        self.event_queue.push_back(Event::AgentStart);
        self.begin_turn(hooks)
    }

    /// Resume after an `Effect::NeedLlm`. Provide the LLM response or error.
    pub fn resume_llm(
        &mut self,
        result: Result<LlmResponse, TransportError>,
        hooks: &mut impl Hooks,
    ) -> Effect {
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
                self.emit_need_llm(hooks)
            }
            Ok(mut resp) => {
                self.consecutive_transport_errors = 0;

                // ── Hook: after_llm ──
                // Hooks can inspect/modify the response before processing.
                hooks.after_llm(&mut resp, &self.state);

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
                    return self.end_turn_and_maybe_continue(hooks);
                }

                if tool_calls.is_empty() {
                    // Model is done for this turn — no tools to run.
                    self.end_turn_and_maybe_continue(hooks)
                } else {
                    // Queue up tool calls.
                    self.pending_tools = tool_calls.into();
                    self.advance_to_next_tool(hooks)
                }
            }
        }
    }

    /// Resume after an `Effect::NeedToolExec`. Provide the execution result.
    pub fn resume_tool(
        &mut self,
        mut result: ToolExecResult,
        hooks: &mut impl Hooks,
    ) -> Effect {
        assert!(matches!(self.phase, Phase::WaitingForTool));

        let tc = self.current_tool.take().expect("no current tool call");

        // ── Hook: after_tool ──
        // Hooks can modify the result before it's recorded.
        let req = ToolCallRequest {
            call_id: tc.id.clone(),
            name: tc.name.clone(),
            arguments: tc.arguments.clone(),
        };
        hooks.after_tool(&req, &mut result, &self.state);

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
        self.advance_to_next_tool(hooks)
    }

    /// Resume after an `Effect::Event`. No data needed — just acknowledgement
    /// that the caller processed the event (updated UI, etc.).
    pub fn resume(&mut self, hooks: &mut impl Hooks) -> Effect {
        assert!(matches!(self.phase, Phase::WaitingForEventAck));
        self.drain_or_advance(hooks)
    }

    // ── Internal state machine logic ─────────────────────────────────

    /// Drain queued events first; if none, advance the FSM.
    fn drain_or_advance(&mut self, hooks: &mut impl Hooks) -> Effect {
        // Always drain events before yielding an I/O effect.
        if let Some(event) = self.event_queue.pop_front() {
            // ── Hook: on_event ──
            hooks.on_event(&event, &self.state);
            self.phase = Phase::WaitingForEventAck;
            return Effect::Event(event);
        }

        // No events — advance based on current phase.
        match &self.phase {
            Phase::Init => self.emit_need_llm(hooks),
            Phase::WaitingForEventAck => {
                self.emit_need_llm(hooks)
            }
            Phase::Done => Effect::Done(EndReason::Done),
            _ => unreachable!("drain_or_advance called in unexpected phase"),
        }
    }

    fn begin_turn(&mut self, hooks: &mut impl Hooks) -> Effect {
        self.state.turn += 1;
        self.event_queue
            .push_back(Event::TurnStart { turn: self.state.turn });
        self.phase = Phase::Init;
        self.drain_or_advance(hooks)
    }

    fn emit_need_llm(&mut self, hooks: &mut impl Hooks) -> Effect {
        // First drain any pending events.
        if let Some(event) = self.event_queue.pop_front() {
            hooks.on_event(&event, &self.state);
            self.phase = Phase::WaitingForEventAck;
            return Effect::Event(event);
        }

        // Build the LLM request.
        let mut req = LlmRequest {
            system_prompt: self.state.system_prompt.clone(),
            messages: self.state.messages.clone(),
            tools: self.tool_schemas.clone(),
        };

        // ── Hook: before_llm ──
        // Hooks can mutate the request: inject context, compact messages,
        // change tool schemas, override model params, etc.
        hooks.before_llm(&mut req, &self.state);

        self.phase = Phase::WaitingForLlm;
        Effect::NeedLlm(req)
    }

    fn advance_to_next_tool(&mut self, hooks: &mut impl Hooks) -> Effect {
        // Drain events first (e.g. ToolExecEnd from previous tool).
        if let Some(event) = self.event_queue.pop_front() {
            hooks.on_event(&event, &self.state);
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
            return self.drain_or_advance_to_llm(hooks);
        }

        // Pop next tool call.
        match self.pending_tools.pop_front() {
            Some(tc) => {
                // Build the tool request.
                let mut req = ToolCallRequest {
                    call_id: tc.id.clone(),
                    name: tc.name.clone(),
                    arguments: tc.arguments.clone(),
                };

                // ── Hook: before_tool ──
                // Hooks can block, replace, or let through.
                let action = hooks.before_tool(&mut req, &self.state);

                match action {
                    ToolAction::Continue => {
                        // Normal path: yield to driver for execution.
                        self.event_queue.push_back(Event::ToolExecStart {
                            call_id: req.call_id.clone(),
                            name: req.name.clone(),
                        });
                        self.current_tool = Some(tc);

                        // Drain the ToolExecStart event first.
                        if let Some(event) = self.event_queue.pop_front() {
                            hooks.on_event(&event, &self.state);
                            self.phase = Phase::WaitingForEventAck;
                            return Effect::Event(event);
                        }

                        self.phase = Phase::WaitingForTool;
                        Effect::NeedToolExec(req)
                    }
                    ToolAction::Block { reason } => {
                        // Hook blocked this tool. Handle internally —
                        // the driver never sees this tool call.
                        let mut result = ToolExecResult::error(
                            format!("blocked: {reason}")
                        );
                        hooks.after_tool(&req, &mut result, &self.state);
                        self.push_tool_result(&tc, result);
                        // Continue to next tool.
                        self.advance_to_next_tool(hooks)
                    }
                    ToolAction::Replace(mut result) => {
                        // Hook provided a result. Handle internally —
                        // the driver never sees this tool call.
                        hooks.after_tool(&req, &mut result, &self.state);
                        self.push_tool_result(&tc, result);
                        // Continue to next tool.
                        self.advance_to_next_tool(hooks)
                    }
                }
            }
            None => {
                // All tools done — loop back to LLM for the next turn.
                self.emit_need_llm(hooks)
            }
        }
    }

    /// After draining events (like Steered), proceed to the LLM call.
    fn drain_or_advance_to_llm(&mut self, hooks: &mut impl Hooks) -> Effect {
        if let Some(event) = self.event_queue.pop_front() {
            hooks.on_event(&event, &self.state);
            self.phase = Phase::WaitingForEventAck;
            return Effect::Event(event);
        }
        self.emit_need_llm(hooks)
    }

    fn end_turn_and_maybe_continue(&mut self, hooks: &mut impl Hooks) -> Effect {
        self.event_queue
            .push_back(Event::TurnEnd { turn: self.state.turn });

        // Check follow-up / steering before declaring done.
        if let Some(steer_msg) = self.steering_queue.pop_front() {
            self.state.messages.push(steer_msg);
            return self.begin_turn_after_events(hooks);
        }

        if let Some(follow_up) = self.follow_up_queue.pop_front() {
            self.state.messages.push(follow_up);
            return self.begin_turn_after_events(hooks);
        }

        // Truly done.
        self.finish(EndReason::Done)
    }

    fn begin_turn_after_events(&mut self, hooks: &mut impl Hooks) -> Effect {
        if let Some(event) = self.event_queue.pop_front() {
            hooks.on_event(&event, &self.state);
            self.phase = Phase::WaitingForEventAck;
            return Effect::Event(event);
        }
        self.begin_turn(hooks)
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
        self.event_queue.push_back(Event::ToolExecEnd {
            call_id: tc.id.clone(),
            result: tool_result.clone(),
        });
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
// The simplest possible driver. Notice the hooks are just passed through
// to every resume call — the driver doesn't know or care what they do.

/// Trait for tools used with `run_blocking`.
pub trait Tool: Send {
    fn schema(&self) -> ToolSchema;
    fn execute(&self, call_id: &str, arguments: &str) -> ToolExecResult;
}

/// Simple blocking driver. Hooks are passed to every step — the agent
/// calls them internally. The driver just does I/O.
pub fn run_blocking(
    messages: Vec<Message>,
    state: AgentState,
    tools: &[&dyn Tool],
    transport: &dyn Fn(&LlmRequest) -> Result<LlmResponse, TransportError>,
    hooks: &mut impl Hooks,
    on_event: &mut dyn FnMut(&Event),
) -> (AgentState, EndReason) {
    let schemas = tools.iter().map(|t| t.schema()).collect::<Vec<_>>();
    let tool_map: std::collections::HashMap<&str, &dyn Tool> = tools
        .iter()
        .map(|t| (t.schema().name.as_str(), *t))
        .collect();

    let mut agent = AgentLoop::new(state, schemas);
    let mut effect = agent.start(messages, hooks);

    loop {
        match effect {
            Effect::Event(ref ev) => {
                on_event(ev);
                effect = agent.resume(hooks);
            }
            Effect::NeedLlm(ref req) => {
                let result = transport(req);
                effect = agent.resume_llm(result, hooks);
            }
            Effect::NeedToolExec(ref req) => {
                let result = match tool_map.get(req.name.as_str()) {
                    Some(tool) => tool.execute(&req.call_id, &req.arguments),
                    None => ToolExecResult::error(format!("unknown tool: {}", req.name)),
                };
                effect = agent.resume_tool(result, hooks);
            }
            Effect::Done(reason) => {
                return (agent.state.clone(), reason);
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// PART 7: Example — async driver with non-blocking tool execution
// ═══════════════════════════════════════════════════════════════════════
//
// Shows: async I/O, user interrupts via tokio::select!, and hooks
// passed through transparently. The driver doesn't do hook logic.

#[cfg(never)]
mod async_example {
    use super::*;

    async fn run_async_agent(
        messages: Vec<Message>,
        state: AgentState,
        tools: &[&dyn Tool],
        hooks: &mut impl Hooks,
    ) -> (AgentState, EndReason) {
        let schemas = tools.iter().map(|t| t.schema()).collect::<Vec<_>>();
        let mut agent = AgentLoop::new(state, schemas);
        let mut effect = agent.start(messages, hooks);

        loop {
            match effect {
                Effect::Event(ref _ev) => {
                    // Log, websocket push, etc.
                    effect = agent.resume(hooks);
                }
                Effect::NeedLlm(ref req) => {
                    let result: Result<LlmResponse, TransportError> =
                        todo!("async_llm_call(req).await");
                    effect = agent.resume_llm(result, hooks);
                }
                Effect::NeedToolExec(ref req) => {
                    let result: ToolExecResult =
                        todo!("async_tool_exec(req).await");
                    effect = agent.resume_tool(result, hooks);
                }
                Effect::Done(reason) => {
                    return (agent.state.clone(), reason);
                }
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// PART 8: Composing multiple hooks
// ═══════════════════════════════════════════════════════════════════════
//
// A single struct that holds multiple hooks and implements Hooks itself.
// This is NOT middleware wrapping the agent — it's just a convenient
// way to combine hooks into one `&mut impl Hooks` value.
//
// The agent doesn't know or care. It calls hooks.before_tool() and
// gets back a ToolAction. Whether that's one hook or ten is invisible.

/// Ordered list of hooks. Runs them in sequence for each interception point.
/// First hook to return Block/Replace wins (short-circuit for before_tool).
/// For mutation methods, all hooks run in sequence, each seeing mutations
/// from previous hooks.
pub struct HookSet {
    hooks: Vec<Box<dyn Hook>>,
}

/// Individual hook — same methods as `Hooks`, but designed to be composed
/// in a `HookSet`. The only difference is that `before_tool` short-circuits:
/// first hook to return non-Continue wins.
pub trait Hook {
    fn name(&self) -> &str { "unnamed" }
    fn before_llm(&mut self, _req: &mut LlmRequest, _state: &AgentState) {}
    fn after_llm(&mut self, _resp: &mut LlmResponse, _state: &AgentState) {}
    fn before_tool(&mut self, _req: &mut ToolCallRequest, _state: &AgentState) -> ToolAction {
        ToolAction::Continue
    }
    fn after_tool(&mut self, _req: &ToolCallRequest, _result: &mut ToolExecResult, _state: &AgentState) {}
    fn on_event(&mut self, _event: &Event, _state: &AgentState) {}
}

impl HookSet {
    pub fn new() -> Self { HookSet { hooks: Vec::new() } }

    pub fn add(&mut self, hook: impl Hook + 'static) {
        self.hooks.push(Box::new(hook));
    }
}

/// HookSet implements Hooks, so it can be passed to any resume_*() call.
impl Hooks for HookSet {
    fn before_llm(&mut self, req: &mut LlmRequest, state: &AgentState) {
        for h in &mut self.hooks { h.before_llm(req, state); }
    }

    fn after_llm(&mut self, resp: &mut LlmResponse, state: &AgentState) {
        for h in &mut self.hooks { h.after_llm(resp, state); }
    }

    fn before_tool(&mut self, req: &mut ToolCallRequest, state: &AgentState) -> ToolAction {
        for h in &mut self.hooks {
            match h.before_tool(req, state) {
                ToolAction::Continue => continue,
                action => return action, // first non-Continue wins
            }
        }
        ToolAction::Continue
    }

    fn after_tool(&mut self, req: &ToolCallRequest, result: &mut ToolExecResult, state: &AgentState) {
        for h in &mut self.hooks { h.after_tool(req, result, state); }
    }

    fn on_event(&mut self, event: &Event, state: &AgentState) {
        for h in &mut self.hooks { h.on_event(event, state); }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// PART 9: Concrete hook examples
// ═══════════════════════════════════════════════════════════════════════
//
// Each hook is self-contained with its own state. They compose by adding
// them to a HookSet. The agent calls them internally — the driver never
// does hook dispatch.

#[cfg(never)]
mod hook_examples {
    use super::*;
    use std::collections::HashMap;

    // ─── Example 1: Approval Gate ────────────────────────────────────
    //
    // The agent calls before_tool() internally. If the hook returns
    // Block, the agent handles it — creates an error result, records it,
    // and moves to the next tool. The driver never sees the blocked call.

    struct ApprovalGate {
        gated_tools: Vec<String>,
        ask: Box<dyn FnMut(&ToolCallRequest) -> bool>,
    }

    impl Hook for ApprovalGate {
        fn name(&self) -> &str { "approval-gate" }

        fn before_tool(
            &mut self,
            req: &mut ToolCallRequest,
            _state: &AgentState,
        ) -> ToolAction {
            if self.gated_tools.iter().any(|t| t == &req.name) {
                if (self.ask)(req) {
                    ToolAction::Continue
                } else {
                    ToolAction::Block {
                        reason: format!("User denied '{}'", req.name),
                    }
                }
            } else {
                ToolAction::Continue
            }
        }
    }

    // ─── Example 2: Metrics Collector ────────────────────────────────
    //
    // Pure observer — never modifies anything. Hook owns its counters.

    struct Metrics {
        turns: usize,
        tool_calls: usize,
        tool_errors: usize,
    }

    impl Hook for Metrics {
        fn name(&self) -> &str { "metrics" }

        fn on_event(&mut self, event: &Event, _state: &AgentState) {
            if let Event::TurnStart { .. } = event {
                self.turns += 1;
            }
        }

        fn after_tool(
            &mut self,
            _req: &ToolCallRequest,
            result: &mut ToolExecResult,
            _state: &AgentState,
        ) {
            self.tool_calls += 1;
            if result.is_error { self.tool_errors += 1; }
        }
    }

    // ─── Example 3: Context Compaction ───────────────────────────────
    //
    // Rewrites the LlmRequest before the agent yields it. The agent
    // builds the request, calls hooks.before_llm(), then yields the
    // (potentially modified) request.

    struct ContextCompactor {
        max_estimated_tokens: usize,
    }

    impl Hook for ContextCompactor {
        fn name(&self) -> &str { "context-compactor" }

        fn before_llm(&mut self, req: &mut LlmRequest, _state: &AgentState) {
            let estimated: usize = req.messages.iter()
                .flat_map(|m| &m.content)
                .map(|b| match b {
                    ContentBlock::Text(t) => t.len() / 4,
                    _ => 50,
                })
                .sum();

            if estimated > self.max_estimated_tokens && req.messages.len() > 4 {
                let len = req.messages.len();
                let mut compacted = Vec::new();
                compacted.extend_from_slice(&req.messages[..1]);
                compacted.push(Message::user(format!(
                    "[{} messages compacted]", len - 4,
                )));
                compacted.extend_from_slice(&req.messages[len - 3..]);
                req.messages = compacted;
            }
        }
    }

    // ─── Composition ─────────────────────────────────────────────────
    //
    // All hooks compose in a HookSet. The driver just passes it through.

    fn example_composition() {
        let mut hooks = HookSet::new();

        // Order matters for before_tool: approval runs first.
        hooks.add(ApprovalGate {
            gated_tools: vec!["bash".into()],
            ask: Box::new(|_req| true),
        });
        hooks.add(ContextCompactor { max_estimated_tokens: 100_000 });
        hooks.add(Metrics { turns: 0, tool_calls: 0, tool_errors: 0 });

        // Now use it — the driver is unchanged:
        //
        //   let mut effect = agent.start(messages, &mut hooks);
        //   loop {
        //       match effect {
        //           Effect::NeedLlm(ref req) => {
        //               let result = transport(req);
        //               effect = agent.resume_llm(result, &mut hooks);
        //           }
        //           Effect::NeedToolExec(ref req) => {
        //               let result = execute(req);
        //               effect = agent.resume_tool(result, &mut hooks);
        //           }
        //           Effect::Event(ref ev) => {
        //               display(ev);
        //               effect = agent.resume(&mut hooks);
        //           }
        //           Effect::Done(reason) => break,
        //       }
        //   }
        //
        // The driver is 12 lines. It doesn't know hooks exist.
        // The agent calls hooks internally and handles Block/Replace itself.
    }
}

// ═══════════════════════════════════════════════════════════════════════
// PART 10: The state machine — how async tool execution works
// ═══════════════════════════════════════════════════════════════════════
//
// The agent loop is a state machine with these phases:
//
//   Init ──┐
//          ▼
//   ┌─→ WaitingForLlm ──→ (process response)
//   │         │
//   │    has tool calls?
//   │    ├── yes ─→ run hooks.before_tool()
//   │    │         ├── Continue ─→ WaitingForTool ──→ (driver runs it)
//   │    │         ├── Block ─→ handle internally, next tool
//   │    │         └── Replace ─→ handle internally, next tool
//   │    │
//   │    └── no ─→ check follow_up / steering ─→ Done or begin_turn
//   │              │
//   │    all tools done ─────────────────────┘
//   │              │
//   └──────────────┘
//
// KEY INSIGHT: when hooks Block or Replace a tool, the state machine
// handles it internally (records the result, advances to next tool)
// WITHOUT yielding to the driver. The driver only sees NeedToolExec
// for tools that actually need external execution.
//
// This means if the LLM calls tools [A, B, C] and the hook blocks A
// and replaces B, the driver only sees one NeedToolExec (for C).
// Tools A and B are handled inside the resume_llm() → advance_to_next_tool()
// chain — invisible to the driver.
//
// For async tool execution, the flow is:
//
//   1. agent yields Effect::NeedToolExec(req)  — "I need this tool run"
//   2. driver starts execution (thread, tokio::spawn, etc. — its choice)
//   3. driver awaits/polls the result
//   4. driver calls agent.resume_tool(result, &mut hooks) — "here's the result"
//   5. agent processes result (hooks.after_tool runs inside), yields next effect
//
// The agent doesn't know or care whether execution was sync or async.
// The "async" part is entirely the driver's problem. This is what makes
// the effect/resume protocol so powerful — the state machine is just:
//   "I need X" → (time passes) → "here's X" → "I need Y" → ...
//
// There's no Future, no Pin, no Poll. The driver bridges to whatever
// execution model it uses.
//
//
// WHY HOOKS ARE PASSED ON EVERY STEP (not stored):
//
//   1. No lifetime entanglement — AgentLoop is 'static, serialisable,
//      cloneable. Storing &mut impl Hooks would infect it with lifetimes.
//
//   2. Hooks own their state — they might hold DB connections, UI handles,
//      file locks. Storing them inside the agent would be bizarre.
//
//   3. The driver controls hook lifetime — create hooks, pass them in,
//      inspect their state between steps, swap them out mid-run.
//
//   4. Zero overhead opt-out — with NoHooks, all hook calls inline to
//      nothing. No vtable, no indirection, no allocation.
//
//   5. Testing — pass mock hooks for one step, real hooks for another.
//      Or pass NoHooks and just test the state machine in isolation.
//
//
// WHAT HOOKS CAN AND CANNOT DO:
//
//   CAN:
//     - Read agent state (messages, turn count, token totals)
//     - Mutate data flowing through (LlmRequest, ToolExecResult, etc.)
//     - Block or replace tool calls (via ToolAction return value)
//     - Observe all events (on_event)
//     - Own arbitrary internal state (counters, caches, connections)
//
//   CANNOT:
//     - Directly mutate agent internal state (messages, phase, queues)
//     - Inject messages into the conversation (that's steer()/follow_up())
//     - Change state machine transitions (the FSM is fixed)
//     - Call async functions (hooks are sync; the driver handles async)
//
// This keeps the agent's behavior predictable: hooks influence the DATA
// flowing through transitions, not the transitions themselves.

fn main() {
    println!("See the code and comments above for the proposed architecture.");
}
