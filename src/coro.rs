//! Coroutine-based async actor handlers with non-borrowing state.
//!
//! # The HRTB Problem
//!
//! Rust's nightly `Coroutine<R>` trait lets you pass arguments on each `resume()`.
//! Ideally, an actor handler coroutine would take `&'a mut State` as its resume
//! argument — receiving a fresh mutable reference on every poll. But the compiler
//! currently can't prove `for<'a> Coroutine<&'a mut State>` even when the reference
//! isn't stored across yield points (rust-lang/rust#68923). This blocks the
//! natural formulation.
//!
//! # The Bypass
//!
//! Instead of `&'a mut State` (which needs a higher-ranked trait bound), we use
//! `StateRef<State>` — a `'static` wrapper around `*mut State`. Since there's no
//! lifetime parameter, the coroutine implements `Coroutine<StateRef<S>>` which is
//! a concrete, monomorphic type. No HRTB needed at all.
//!
//! The safety invariant (StateRef is only valid during one resume segment) is
//! enforced by:
//! - The `co_await!` macro, which rebinds the StateRef after each yield
//! - Debug-mode generation counters that panic on stale access
//! - The `CoroutineHandler` wrapper, which controls pointer creation/invalidation
//!
//! # Architecture
//!
//! ```text
//! ┌──────────────────────────────────────────────────┐
//! │ CoroutineHandler<C, S, R>  (implements Future)   │
//! │                                                  │
//! │  ┌─────────┐     resume(StateRef)    ┌────────┐  │
//! │  │  state   │──── *mut ──────────────▶│ coro   │  │
//! │  │  (owned) │                        │        │  │
//! │  └─────────┘     yield sub_future    │        │  │
//! │       ▲          ◀───────────────────│        │  │
//! │       │                              └────────┘  │
//! │  ┌────┴──────┐                                   │
//! │  │sub_future │  (polled until Ready, then        │
//! │  │(optional) │   coroutine is resumed again)     │
//! │  └───────────┘                                   │
//! └──────────────────────────────────────────────────┘
//! ```
//!
//! # Comparison with current `ak` approach
//!
//! | Aspect              | `#[suspend]` proc macro        | Coroutine + `co_await!`       |
//! |---------------------|--------------------------------|-------------------------------|
//! | Code rewriting      | Proc macro rewrites `.await`   | Simple `macro_rules!`         |
//! | Explicitness        | Magic — `.await` does more     | Explicit `co_await!` call     |
//! | Debug validation    | None                           | Generation counter in debug   |
//! | Nightly features    | `type_alias_impl_trait`        | `coroutines`, `coroutine_trait` |
//! | State handle        | `ContextRef<A>` (raw ptr)      | `StateRef<S>` (raw ptr)       |
//! | Sub-future results  | Returned from `suspend()`      | Written to `Rc<Cell>` slot    |

#![allow(unused)]

use std::cell::Cell;
use std::future::Future;
use std::marker::PhantomData;
use std::ops::{Coroutine, CoroutineState, Deref, DerefMut};
use std::pin::Pin;
use std::rc::Rc;
use std::task::{self, Poll};

// ============================================================================
// StateRef: the 'static pointer wrapper that bypasses HRTB
// ============================================================================

/// A `'static` mutable handle to actor state.
///
/// This is the key to bypassing rust-lang/rust#68923: instead of requiring
/// `for<'a> Coroutine<&'a mut S>` (which the compiler can't prove), we use
/// `Coroutine<StateRef<S>>` where `StateRef<S>: 'static`.
///
/// # Safety contract
///
/// A `StateRef` is valid only during the coroutine resume segment in which it
/// was created. After the coroutine yields or returns, the `StateRef` is stale.
/// The `co_await!` macro enforces this by rebinding the identifier after yield.
/// In debug builds, a generation counter catches stale access with a panic.
pub struct StateRef<S> {
    ptr: *mut S,
    #[cfg(debug_assertions)]
    generation: u64,
    #[cfg(debug_assertions)]
    valid_generation: *const u64,
}

// StateRef is 'static — that's the whole point. No lifetime parameter.
// This is what makes `Coroutine<StateRef<S>>` work without HRTB.

impl<S> Deref for StateRef<S> {
    type Target = S;

    #[inline]
    fn deref(&self) -> &S {
        #[cfg(debug_assertions)]
        {
            assert_eq!(
                self.generation,
                unsafe { *self.valid_generation },
                "StateRef used after yield point — this is a bug! \
                 The StateRef must be rebound via co_await!() after each suspension."
            );
        }
        unsafe { &*self.ptr }
    }
}

impl<S> DerefMut for StateRef<S> {
    #[inline]
    fn deref_mut(&mut self) -> &mut S {
        #[cfg(debug_assertions)]
        {
            assert_eq!(
                self.generation,
                unsafe { *self.valid_generation },
                "StateRef used after yield point — this is a bug! \
                 The StateRef must be rebound via co_await!() after each suspension."
            );
        }
        unsafe { &mut *self.ptr }
    }
}

// ============================================================================
// Type alias for yielded sub-futures
// ============================================================================

/// The type yielded by handler coroutines: a type-erased, boxed sub-future.
///
/// When the coroutine wants to "await" an async operation, it yields one of
/// these. The `CoroutineHandler` wrapper polls it to completion, then resumes
/// the coroutine with a fresh `StateRef`.
pub type SubFuture = Pin<Box<dyn Future<Output = ()>>>;

/// Helper to coerce `impl Future<Output=()>` into the erased `SubFuture` type.
/// Needed because coroutine yield expressions must all produce the same type.
#[inline]
pub fn sub_future(f: impl Future<Output = ()> + 'static) -> SubFuture {
    Box::pin(f)
}

// ============================================================================
// co_await! — the yield-and-rebind macro
// ============================================================================

/// Await an async operation inside a coroutine handler.
///
/// This macro:
/// 1. Creates a result slot (`Rc<Cell<Option<T>>>`)
/// 2. Wraps the future to write its result into the slot on completion
/// 3. Yields the wrapped future (coroutine suspends)
/// 4. Rebinds the state variable from the new resume argument
/// 5. Returns the result from the slot
///
/// The state variable is automatically rebound after each yield, so it always
/// points to valid, current state. Old `StateRef` values become invalid.
///
/// # Example
///
/// ```ignore
/// #[coroutine]
/// move |mut state: StateRef<MyActor>| {
///     state.count += 1;
///     let resp: Response = co_await!(state, fetch_data(state.id));
///     state.count += resp.delta;
/// }
/// ```
///
/// # Important
///
/// Any data borrowed from `state` that the sub-future needs must be
/// copied/cloned *before* the `co_await!`, since `state` is invalidated
/// at the yield point. The macro evaluates `$fut` eagerly (before yielding),
/// so `co_await!(state, fetch(&state.name))` is fine — `fetch()` is called
/// and captures what it needs before the yield.
#[macro_export]
macro_rules! co_await {
    ($state:ident, $fut:expr) => {{
        // Slot to receive the sub-future's result. Lives in coroutine state
        // across the yield (it's an Rc, not a borrow — no self-ref issue).
        let __slot = ::std::rc::Rc::new(::std::cell::Cell::new(None));
        let __slot_writer = __slot.clone();

        // Evaluate the future expression NOW (before yield). This lets it
        // capture borrows from the current state, which are valid until yield.
        let __fut = $fut;

        // Yield a sub-future that runs __fut and writes result to the slot.
        // The yield expression evaluates to the next resume arg (fresh StateRef).
        $state = yield $crate::coro::sub_future(async move {
            __slot_writer.set(Some(__fut.await));
        });

        // Read the result. The sub-future has completed (CoroutineHandler
        // only resumes us after the sub-future is Ready).
        __slot.take().expect("sub-future completed but didn't write result")
    }};
}

/// Yield without awaiting a sub-future. Just suspends and gets a fresh StateRef.
///
/// Useful for cooperative yielding or when state was modified and you want to
/// give the event loop a chance to process other work.
#[macro_export]
macro_rules! co_yield {
    ($state:ident) => {{
        // Yield an immediately-ready future.
        $state = yield $crate::coro::sub_future(::std::future::ready(()));
    }};
}

// ============================================================================
// CoroutineHandler: the Future adapter
// ============================================================================

/// Wraps a coroutine + owned state into a `Future`.
///
/// The state `S` lives here, NOT inside the coroutine. The coroutine only
/// receives temporary `StateRef<S>` access during each resume segment.
/// This is what makes the resulting future non-borrowing and `'static`.
///
/// # Polling protocol
///
/// ```text
/// poll() called
///   │
///   ├─ sub_future pending? ──▶ poll sub_future ──▶ Pending? return Pending
///   │                                             Ready?  clear sub_future
///   │                                                     ↓
///   ├─ resume coroutine with StateRef ◀───────────────────┘
///   │     │
///   │     ├─ Yielded(sub_future) ──▶ store sub_future, loop back to top
///   │     │
///   │     └─ Complete(result) ──▶ return Ready(result)
/// ```
pub struct CoroutineHandler<C, S, R> {
    coroutine: C,
    state: S,
    sub_future: Option<SubFuture>,
    #[cfg(debug_assertions)]
    generation: u64,
    _result: PhantomData<R>,
}

impl<C, S, R> CoroutineHandler<C, S, R>
where
    C: Coroutine<StateRef<S>, Yield = SubFuture, Return = R>,
{
    /// Create a new handler from owned state and a coroutine.
    ///
    /// The coroutine should be created with `#[coroutine] move |mut state: StateRef<S>| { ... }`.
    pub fn new(state: S, coroutine: C) -> Self {
        CoroutineHandler {
            coroutine,
            state,
            sub_future: None,
            #[cfg(debug_assertions)]
            generation: 0,
            _result: PhantomData,
        }
    }

    fn make_state_ref(&mut self) -> StateRef<S> {
        StateRef {
            ptr: &mut self.state as *mut S,
            #[cfg(debug_assertions)]
            generation: self.generation,
            #[cfg(debug_assertions)]
            valid_generation: &self.generation as *const u64,
        }
    }

    fn invalidate_state_refs(&mut self) {
        #[cfg(debug_assertions)]
        {
            self.generation = self.generation.wrapping_add(1);
        }
    }
}

// CoroutineHandler is !Unpin (coroutines are !Unpin). This is important:
// once pinned, `self.state` won't move, so StateRef pointers remain valid.
// Sub-futures may also borrow from state (via pointers created before yield),
// and pinning guarantees those won't dangle.

impl<C, S, R> Future for CoroutineHandler<C, S, R>
where
    C: Coroutine<StateRef<S>, Yield = SubFuture, Return = R>,
{
    type Output = R;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<R> {
        // SAFETY: We never move any field out of `self`. The struct is !Unpin
        // (because C is a coroutine, which is !Unpin), so callers can't get
        // &mut Self to move fields either. Both `state` and `coroutine` stay
        // at their pinned addresses for the lifetime of the CoroutineHandler.
        let this = unsafe { self.get_unchecked_mut() };

        loop {
            // Phase 1: If a sub-future is pending, poll it.
            if let Some(ref mut fut) = this.sub_future {
                match fut.as_mut().poll(cx) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(()) => {
                        this.sub_future = None;
                        // Fall through to resume the coroutine.
                    }
                }
            }

            // Phase 2: Invalidate old StateRefs (debug: bumps generation).
            this.invalidate_state_refs();

            // Phase 3: Resume coroutine with a fresh StateRef.
            let state_ref = this.make_state_ref();
            // SAFETY: The coroutine is structurally pinned (we never move it).
            let coro_pin = unsafe { Pin::new_unchecked(&mut this.coroutine) };

            match coro_pin.resume(state_ref) {
                CoroutineState::Yielded(fut) => {
                    // Coroutine suspended — it wants this sub-future polled.
                    this.sub_future = Some(fut);
                    // Loop back to poll the sub-future immediately (it might
                    // be ready already, e.g. `std::future::ready(())`).
                    continue;
                }
                CoroutineState::Complete(result) => {
                    return Poll::Ready(result);
                }
            }
        }
    }
}

// ============================================================================
// Convenience constructor
// ============================================================================

/// Create a `CoroutineHandler` future from state and a coroutine-producing closure.
///
/// # Example
///
/// ```ignore
/// let future = coro::handler(my_actor_state, |msg| {
///     #[coroutine]
///     move |mut state: StateRef<MyActor>| {
///         state.handle(msg);
///         let result: Resp = co_await!(state, some_rpc(state.id));
///         state.apply(result);
///     }
/// });
/// ```
pub fn handler<S, R, C>(state: S, coro: C) -> CoroutineHandler<C, S, R>
where
    C: Coroutine<StateRef<S>, Yield = SubFuture, Return = R>,
{
    CoroutineHandler::new(state, coro)
}

// ============================================================================
// Integration sketch: how this replaces the current ak Handler trait
// ============================================================================

// The current ak Handler trait:
//
//   pub trait Handler<M: Message>: Actor {
//       type Future: Future<Output=M::Result> + 'static;
//       fn handle(self: ContextRef<Self>, msg: M) -> Self::Future;
//   }
//
// With the #[suspend] proc macro rewriting .await expressions to thread
// ContextRef through suspend() calls.
//
// The coroutine-based equivalent:
//
//   pub trait Handler<M: Message>: Actor {
//       type Coro: Coroutine<StateRef<Self>, Yield = SubFuture, Return = M::Result>;
//       fn handle(msg: M) -> Self::Coro;
//   }
//
// The Context event loop would wrap Self::Coro in a CoroutineHandler,
// passing &mut self.actor as the state. No proc macro needed.
//
// Example handler implementation:
//
//   impl Handler<Payload> for Node {
//       type Coro = impl Coroutine<StateRef<Self>, Yield = SubFuture, Return = ()>;
//
//       fn handle(msg: Payload) -> Self::Coro {
//           #[coroutine]
//           move |mut state: StateRef<Node>| {
//               if msg.0 >= state.limit {
//                   // Direct state access, no indirection
//                   state.stop();
//                   return;
//               }
//               if let Some(next) = &state.next {
//                   let _: () = co_await!(state, next.send(Payload(msg.0 + 1)));
//               }
//           }
//       }
//   }
//
// Key differences from current approach:
// - No #[suspend] proc macro — just co_await!() which is a simple macro_rules!
// - No ContextRef raw pointer wrapper — StateRef is semantically identical but
//   has debug-mode validation
// - Handler::handle doesn't take self — it returns a coroutine that receives
//   state on each resume
// - The CoroutineHandler Future adapter replaces the suspend() async fn
