# Host families and scoped coroutine resumption

This independent, dependency-free experiment checks whether a single family-based
call protocol can serve both ordinary hosts and temporary composed hosts. It does
not change the public crates or the call declaration syntax.

The public implementation now uses this family-based direction. See
[`nitori_call`](../../crates/nitori_call/README.md) for `Host::view`, real receivers,
Arguments, and author-defined composition, and the
[composition tests](../../crates/nitori_io/tests/composition.rs) for macro-driven
nested calls. This standalone package retains the focused compiler controls and
rejected alternatives; its experimental names are not the public API.

## Protocol

`Family::Host<'host>` describes a sized host access view for a particular visit.
`Root<H>::Host<'host> = Pin<&'host mut H>` handles ordinary, possibly unsized hosts
without owning or moving H. A persistent `Call<F>` accepts a fresh view on every
poll, with independent outer access and inner host lifetimes. Neither Yield nor
Return depends on the visit lifetime. Long-lived input borrows may still appear
in the family and the output.

`Fixed<H>` remains a constant-family control for sized H. It uses the same Call
protocol, but has no generic reconstruction implementation: an arbitrary H cannot
be duplicated from a borrow. Composable ordinary-host entry points use `Root<H>`.
The sized-view requirement does not require an unsized underlying resource to
become sized; the pinned reference is itself the sized view.

The fixed coroutine resume argument dispatches through a pointer to a live stack
slot and a lifetime-specialized function pointer. The slot preserves the original
host type, including invariant lifetime parameters and unsized pointer metadata.
An object-safe request with a lifetime-generic method executes at that original
lifetime. No host is cast to `Host<'static>` or assumed covariant. Requests use
dynamic dispatch, but no heap allocation; this is a feasibility implementation,
not a performance benchmark. The public runtime contract is documented in
`nitori_call`.

The unsafe boundary resembles the existing coroutine runtime: each environment
must be consumed before suspension and never accessed through an escaped value or
Drop. This experiment uses manually written coroutine bodies satisfying that
contract. It does not prove that a revised procedural macro enforces it for all
accepted syntax. Miri checks the exercised paths, not general soundness.

`ReadFamily` explicitly lifts a small read capability and its stable error type.
Automatic capability-bound translation, real receiver methods, Arguments, and
call signature syntax are deliberately outside this experiment.

## Passing behavior

Ten execution tests cover:

- Constant families, non-static inputs and borrowed error returns.
- Compiler-generated persistent coroutines with nested child operations,
  Pending/wakeup, intermediate yields, and completion.
- Author-defined wrappers invariant in the visit lifetime, reconstructed between
  resumes of the same coroutine.
- Non-Send and pinned `!Unpin` hosts and local wrapping state.
- A parent coroutine retaining local state and composing its fixed host for a
  child coroutine.
- Two nested views constructed together from the root, preserving both states.
- Unsized host metadata, cancellation, unwinding inside dispatch, and poisoning
  after panic.
- A child creating a pinned local state after resumption and adding a second
  layer to an existing invariant family host. The generic composition adapter
  reconstructs the inner view; layers are not all prebuilt at the root.
- Nominal with callbacks capturing mutable locals and returning non-static data
  from host state. Temporary host references still cannot escape.
- Author-defined view capabilities, including an added tagging trait, and
  cancellation dropping each nested state exactly once without resetting IO.

Four compiler safety cases reject returning or retaining a temporary host borrow
through either direct polling or the nominal with callback.

## Nested composition through explicit reconstruction

Building a nested view tree from the root is not equivalent to an arbitrary child
adding another local layer to its already materialized family host.

The child receives:

```text
Pin<&'access mut F::Host<'host>>
```

Adding state borrowed for `'access` gives a natural layer with two independent
lifetimes. The initially proposed one-parameter family instead asks for:

```text
Layer<'access, F> containing Pin<&'access mut F::Host<'access>>
```

This requires replacing the inner `'host` with `'access`. That replacement is not
generally valid. [The generic reproduction](probes/nested_compose.rs) fails, and
[the concrete reproduction](probes/nested_concrete.rs) shows the same issue without
GATs using an invariant `Cell<&mut _>`. A
[constant-family control](probes/nested_fixed.rs) passes with borrowed input.

The implemented solution is [Reborrow](src/lending.rs):

```text
Pin<&'short mut F::Host<'long>>
    -> newly constructed F::Host<'short>
```

It constructs another access view of the same resources. It does not coerce the
old object to another lifetime, move a pinned inner object, or duplicate/reset
persistent state. Root reborrows its pinned reference. Each author-defined layer
reborrows the inner view and its separately owned state, then constructs its own
short view type. Even lifetime-invariant, !Unpin views can implement this contract
by projecting and rebuilding; the tests exercise both properties.

The new outer layer **owns the temporary inner view value**, rather than storing
a reference to the old inner view object. It only borrows persistent state and
resources. Consequently all components of the new view can use the same short
visit lifetime, while the old view and its original inner lifetime remain intact.
On suspension the temporary views are dropped, leaving each coroutine's state in
place. This can repeat at each call depth.

Layer authors provide the associated view type plus compose and reborrow
implementations through `Layer<F>`. This is an explicit semantic contract, not an
automatic blanket implementation for arbitrary existing wrappers. Reconstructing
views must preserve the same logical resources and all persistent progress.
Opaque wrappers without a suitable projection/reconstruction interface need an
adapter or cannot participate in this composition mechanism. Ownership-returning
state policies are not selected by this borrowed-state experiment.

## Nominal with callback argument

[The naive callback bound](probes/with_borrowed.rs),
`for<'host> FnOnce(Pin<&mut F::Host<'host>>)`, unexpectedly requires a non-static
constant host to be static on the tested compiler. Its diagnostic explicitly
identifies a current type-system limitation.

[A control](probes/with_fixed.rs) fixes the inner host lifetime and quantifies only
the outer access lifetime, but is insufficient for a persistent operation.

The implemented with call quantifies a nominal argument instead:

```text
for<'access, 'host> FnOnce(Access<'access, 'host, F>) -> R
```

Access contains the actual pinned view and exposes it through `into_pin`. This
retains independent access/host lifetimes and avoids quantifying the GAT
projection directly in the Fn bound. It performs no lifetime cast. The
[non-static control](probes/with_nominal_borrowed.rs) accepts mutable local
captures and borrowed output; the two with escape probes reject exporting the
temporary host borrow, including through a captured variable.

The operation invokes the closure synchronously once and returns Ready. It can
be driven by the same family coroutine dispatcher as any other child operation;
no special host-access path is required. Surface receiver methods and `.await`
lowering remain part of the separate macro/API design.

## Reproduce

Prerequisites: Python 3 and Rust nightly with rustfmt and Clippy. Optional Miri runs
require the Miri component. Checked with rustc 1.100.0-nightly (feaadeeac,
2026-09-19).

From the nitori repository root:

```sh
python3 experiments/host-family/verify.py --miri
```

The runner checks formatting, Clippy, execution tests, three successful compiler
controls, four safety rejections, and three rejected-alternative reproductions. It prints
and retains a temporary directory containing compiler diagnostics. With `--miri`,
execution tests also run under both default and Tree Borrows models.

The rejected alternatives are retained to document why coercing arbitrary views
or directly quantifying GAT projections is not the implemented design. Passing
them means the expected rejection is reproduced, not that these alternatives
compile. The positive execution tests exercise their implemented replacements.
This isolated package has no external dependencies; workspace-local Cargo patches
may emit unused-patch warnings.
