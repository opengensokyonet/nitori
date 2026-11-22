# nitori_driver

The tree of state a driver keeps and lends to its calls. It is experimental
and builds without the standard library, with `alloc`; it needs the nightly
toolchain the repository pins.

A driver holds the whole state of one connection, or of another protocol's
connection, and advances many calls in turn. This crate gives the mechanism
without protocol meaning; the code that uses it declares its own structure,
parts and calls.

- `structure`: node kinds declare their children (a kind and a storage
  kind each) and the nodes they link to; a node holds the parts' shares
  and the stores of its children. Typed paths and the driver's addresses
  follow from the kinds alone, so they name no part. Nodes are added,
  removed and moved, and removed in an order where children go before
  their parent and a linked node after the nodes that link to it.
- `need` and `lend`: what a call asks of the tree around its node, as a
  list of needs (an anchor on the way up, steps down, a component taken
  exclusive or shared, or the children of a store by key), proved where the
  structure is concrete. Needs that overlap do not compile; whether a node
  reached through an at-most-one child or by key is there is an `Option`
  at run time.
- `call`: a call is a `#[call]` function of `nitori_call`, generic over
  its family; the parts' shares list the calls, and a composition proves
  every call of the tree once. Each poll lends a call a new view of its
  needs.
- `capability`: composed capabilities (a list of needs and the view made
  of theirs) and contributed ones (every part's piece for a family, in part
  order, dispatched by a sum or run in turn).
- `hooks`: what the driver calls on every component of a node, with the
  component as `Self`: associated functions lent the component and its own
  needs, `build` lent the parent's, every part's construction arguments,
  and a deadline for a component that writes `Timed`.
- `event`: events are types, published through a need at a node on the way
  up and heard by anchor and steps, or by default from the node, the nodes
  above it and below it.
- `driver`: the future that runs a composition: the places of the tree and
  every hook proved at its place, waking without a lock, the ready nodes'
  calls polled, deadlines last, events handed on until nothing more is
  published, nodes built and ended in the order of removal.

The requests that change the structure are not here yet (`request`).

The crate that writes a composition enables the nightly feature
`impl_trait_in_assoc_type`; a `#[call]` function that yields needs
`yield_expr`. The crate re-exports `frunk_core`, whose lists and sums the
structure, the needs and the views are made of.
