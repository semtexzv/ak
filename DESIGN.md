# Fixing `for<'a> Coroutine<&'a mut T>` in rustc

## Problem Statement

Rust's nightly `Coroutine<R>` trait supports resume arguments — values passed
into the coroutine on each `resume()` call. When `R` contains a lifetime (e.g.
`&'a mut T`), the compiler infers a **specific** lifetime `'x` rather than the
higher-ranked `for<'x> Coroutine<&'x mut T>`, even when the reference is never
stored across a yield point.

```rust
// This coroutine never holds `arg` across the yield.
// It SHOULD implement for<'x> Coroutine<&'x mut bool>.
// It actually implements Coroutine<&'specific mut bool>.
let coro = #[coroutine] |arg: &mut bool| {
    yield ();
    *arg = true;
};
```

Tracking issue: [rust-lang/rust#68923](https://github.com/rust-lang/rust/issues/68923)

Blocked by: [rust-lang/rust#69663](https://github.com/rust-lang/rust/issues/69663) (more precise coroutine captures)

## Motivation

The primary use case is **non-borrowing async state machines** — actor handlers,
protocol state machines, event-driven systems — where mutable state is passed
*in* on each poll rather than captured inside the future.

```rust
// Actor handler: state is passed in, not captured.
// The resulting future is 'static and non-self-referential.
#[coroutine]
move |mut ctx: &mut ActorState| {
    ctx.count += 1;
    yield sub_future;          // ctx dropped here
    ctx.count -= 1;            // ctx re-supplied by caller
}
```

Without HRTB, this coroutine can only accept `&'one_specific_lifetime mut ActorState`,
making it useless for the pattern where each `resume()` passes a reference with
a potentially different lifetime.

## Root Cause Analysis

The limitation is **structural**, not an inference bug. Three sites conspire:

### 1. `CoroutineArgs.resume_ty` is a flat `Ty`

`compiler/rustc_type_ir/src/ty_kind/closure.rs`:

```rust
pub struct CoroutineArgsParts<I: Interner> {
    pub parent_args: I::GenericArgsSlice,
    pub kind_ty: I::Ty,
    pub resume_ty: I::Ty,          // flat type — no binder
    pub yield_ty: I::Ty,
    pub return_ty: I::Ty,
    pub tupled_upvars_ty: I::Ty,
}
```

Compare with closures, which store their signature as `FnPtr` wrapping
`PolyFnSig` — a `Binder<FnSig>`. The binder is what carries `for<'a>`.
Coroutines have **no binder** anywhere in their type representation.

### 2. `liberate_late_bound_regions` strips the binder before storage

`compiler/rustc_hir_typeck/src/closure.rs`, in `closure_sigs()`:

```rust
let liberated_sig = self.tcx().liberate_late_bound_regions(
    expr_def_id.to_def_id(),
    bound_sig,
);
```

Then in `check_expr_closure`, the Coroutine branch extracts:

```rust
let resume_ty = liberated_sig.inputs().get(0).copied().unwrap_or(tcx.types.unit);
```

The `for<'a>` has already been replaced with a concrete free region variable.
This gets baked into `CoroutineArgs::new()`. The universally-quantified nature
of the lifetime is **permanently lost**.

### 3. The trait solver equates `R` directly — no binder

`compiler/rustc_next_trait_solver/src/solve/trait_goals.rs`,
`consider_builtin_coroutine_candidate`:

```rust
let coroutine = args.as_coroutine();
Self::probe_and_consider_implied_clause(
    ecx, ..., goal,
    ty::TraitRef::new(cx, goal.predicate.def_id(),
        [self_ty, coroutine.resume_ty()]),   // flat, no binder
    [],
)
```

Compare with closures in `consider_builtin_fn_trait_candidates`:

```rust
// Closures: extract binder, then instantiate
let (inputs, output) = ecx.instantiate_binder_with_infer(tupled_inputs_and_output);
```

Closures work through a binder via `instantiate_binder_with_infer`. Coroutines
don't have a binder to instantiate.

## Proposed Fix: Three Levels

### Level 1 — Structural: Wrap resume_ty in a Binder

**Difficulty: Medium. Estimated: 2-4 weeks for an experienced compiler contributor.**

Change `CoroutineArgs` to store the coroutine signature inside a binder,
analogous to how closures store `PolyFnSig`:

```rust
// Before:
pub resume_ty: I::Ty,
pub yield_ty: I::Ty,
pub return_ty: I::Ty,

// After: single bound signature (like GenSig but inside a Binder)
pub bound_coroutine_sig: I::Ty,  // encoding Binder<GenSig>
```

Then update the trait solver to instantiate through the binder:

```rust
// Before:
[self_ty, coroutine.resume_ty()]

// After:
let sig = ecx.instantiate_binder_with_infer(coroutine.bound_sig());
[self_ty, sig.resume_ty]
```

**Scope**: `resume_ty()`, `yield_ty()`, `return_ty()` are accessed in ~50-80
files across `rustc_hir_typeck`, `rustc_trait_selection`,
`rustc_next_trait_solver`, `rustc_type_ir`, and `rustc_mir_transform`. Every
callsite must handle the binder. This is mechanical — the closure code is
a direct template — but it's a wide-reaching refactor.

**Not blocked on anything.** This could be done today as a prerequisite.

### Level 2 — Soundness: Determine when HRTB is safe

**Difficulty: Hard. Blocked on [#69663](https://github.com/rust-lang/rust/issues/69663).**

The binder from Level 1 isn't sufficient alone. The compiler must only produce
`for<'a>` when the resume arg's lifetime is NOT stored across a yield point.
If the coroutine holds `&'a mut T` across a yield, HRTB would be unsound
because the caller could pass a short-lived reference that the coroutine
outlives.

The information about what survives across yields already exists — in MIR:

```
compiler/rustc_mir_transform/src/coroutine.rs
    fn locals_live_across_suspend_points(...)
```

But it's computed **after** type checking has already committed to concrete
types. This is a **phase ordering problem**: the type system needs information
from MIR, but MIR is built after typeck.

**Possible approaches:**

| Approach | Description | Trade-off |
|----------|-------------|-----------|
| **Conservative pessimistic** | Always allow `for<'a>` in the type, then reject in borrowck if the resume arg is stored across a yield | Simple to implement, may reject valid programs initially. Analogous to how closure captures worked before 2021-edition precise captures |
| **Use CoroutineWitness** | The witness type already tracks live-across-yield data with higher-ranked lifetimes. If the resume arg appears in the witness, the lifetime is early-bound (no HRTB). If absent, late-bound (HRTB is sound) | Correct but couples typeck to MIR analysis. Same phase-ordering problem as #69663 |
| **Syntactic conservative check** | At the HIR level, check whether any local binding derived from the resume arg is live across a `yield`. Over-approximate | Simple, no phase ordering issue, but rejects some valid programs |

### Level 3 — Semantics: Distinct lifetimes per yield point

**Difficulty: Very hard. No existing design.**

A coroutine with multiple yields needs **distinct** universally-quantified
lifetimes at each resume point:

```rust
#[coroutine]
|first: &'a mut T| {
    let second: &'b mut T = yield v1;  // 'b != 'a
    let third: &'c mut T = yield v2;   // 'c != 'b != 'a
}
```

The current `Coroutine<R>` trait has a single `R`. If `R = &'a mut T`, there is
ONE lifetime for all resume points. But soundness requires that each yield
introduces a **fresh** lifetime — otherwise `mem::swap(first, second)` would be
accepted despite the references originating from different `resume()` calls
with potentially non-overlapping lifetimes.

This requires either:

- **N lifetimes for N yields**: `for<'a, 'b, 'c> Coroutine<&'? mut T>` — the
  trait doesn't support this and can't (yield count is an implementation detail)
- **Per-yield skolemization in borrowck**: `for<'a> Coroutine<&'a mut T>` with
  the understanding that borrowck treats each yield as introducing a fresh
  instantiation of `'a`. This is the most promising direction but requires NLL
  extensions that don't exist yet

This is genuinely **research-level** — no RFC or design document addresses it.

## Pragmatic Subset Fix

A useful first step that avoids Level 3 entirely:

> Support `for<'a> Coroutine<&'a mut T>` **only** when the resume argument
> is never live across any yield point.

This means every use of the resume arg must be **consumed** (moved, copied, or
dropped) before each `yield`. The coroutine effectively treats the resume arg
as a "per-segment parameter" that doesn't persist.

Under this restriction:
- Level 1 (structural binder change) is needed
- Level 2 becomes simple: "is the resume arg in the witness? → no HRTB"
- Level 3 doesn't apply: no resume arg survives across yields, so there are
  no distinct lifetimes to reason about

This covers the **primary use case** (actor handlers, async state machines)
where the pattern is:

```rust
#[coroutine]
|mut state: &mut ActorState| {
    state.process();
    // state is consumed/dropped before yield:
    let state: &mut ActorState = yield sub_future;
    state.finalize();
}
```

## Complexity Summary

| Component | Difficulty | Blocked on | Estimated effort |
|-----------|-----------|------------|-----------------|
| Wrap resume_ty in a Binder | Medium | Nothing | 2-4 weeks |
| Sound HRTB (conservative) | Hard | [#69663](https://github.com/rust-lang/rust/issues/69663) | Months |
| Distinct per-yield lifetimes | Very hard | NLL extensions | Research-level |
| Pragmatic subset (no storage across yield) | Medium-Hard | Level 1 only | 1-2 months |

## Relevant Compiler Files

```
compiler/rustc_type_ir/src/ty_kind/closure.rs    — CoroutineArgsParts definition
compiler/rustc_hir_typeck/src/closure.rs          — closure_sigs(), check_expr_closure()
compiler/rustc_hir_analysis/src/collect/generics_of.rs — coroutine generic params
compiler/rustc_next_trait_solver/src/solve/trait_goals.rs — consider_builtin_coroutine_candidate
compiler/rustc_next_trait_solver/src/solve/assembly/structural_traits.rs — extract_tupled_inputs_*
compiler/rustc_trait_selection/src/traits/util.rs — coroutine_trait_ref_and_outputs
compiler/rustc_type_ir/src/outlives.rs            — resume arg outlives (PR #132151)
compiler/rustc_mir_transform/src/coroutine.rs     — locals_live_across_suspend_points
```

## References

- [#68923 — Coroutines should support `for<'a> Coroutine<&'a mut T>`](https://github.com/rust-lang/rust/issues/68923)
- [#69663 — More precise coroutine captures](https://github.com/rust-lang/rust/issues/69663)
- [#132151 — Ensure resume arg outlives region bound](https://github.com/rust-lang/rust/pull/132151)
- [#43122 — Tracking issue for RFC 2033 (experimental coroutines)](https://github.com/rust-lang/rust/issues/43122)
- [RFC 2781 — Unified coroutines / generator resume arguments](https://github.com/rust-lang/rfcs/pull/2781)
- [Lang team design notes — Generalizing coroutines](https://lang-team.rust-lang.org/design_notes/general_coroutines.html)
