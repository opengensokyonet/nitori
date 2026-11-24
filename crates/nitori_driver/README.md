# nitori_driver

A driver for the state of one connection, or of another protocol's
connection: a tree of nodes the code that uses it declares, the calls it
runs at the nodes by priority, and what it lends them. It is experimental
and builds without the standard library, with `alloc`, when its default
`std` feature is off; it needs the nightly toolchain the repository pins.
The `std` feature turns on `nitori_call`'s, whose call slots are then
`Send`: a driver is `Send` when what it holds is, the components, calls,
requests and keys, the clock and the parts' arguments.

This crate gives the mechanism without protocol meaning; the code that
uses it declares its structure, its parts, their components and calls,
and what each kind of node can be asked.

- `structure`: node kinds declare their children (a kind and a storage
  kind each), what a new node starts from, why it ends, and the request it
  handles. A kind is at one place of the structure only. Each part keeps a
  share in each kind of node: components, the calls that run there, and
  its reactions. Key addresses follow from the kinds alone.
- `need`: what is taken from the tree around a lend point, in terms of
  components: on the way up to the root (`Mut`, `Ref`), at every node of
  one kind below an anchor (`Under`), from another lend point (`At`, or a
  `Locator`), one member at a time (`OneAtATime`), and the lend point's
  own key and address (`OwnKey`, `OwnAddress`); from the lending, the
  clock (`Clock`), the request boxes (`Requests`) and component wakes
  (`WakerOf`, `Wakes`). A list of needs is proved where the structure is
  known: a component no node or more than one node has, and two needs that
  overlap, do not compile, and the error says why. A view's type follows
  from the needs, so code generic over the structure names no position.
- `component`: a component's operations, `build` when its node is added,
  `wake` when its waker is woken and `end` when the node ends, each lent
  its own needs.
- `capability` and `react`: capabilities put together from needs, or
  contributed by every part; reaction points that the other parts react
  to where an action happens, among them the driver's `NodeStarted` and
  `NodeEnding`.
- `call`: kinds of calls (`CallKind`, written with `nitori_call`'s
  `#[call]`), lent their needs on every poll; a call sets the priority its
  standard waker wakes it at, and `settle`s to wait for the requests it
  put in. A composition (`composition!`) names a root kind, the parts, a
  clock and a priority type, and proves every call, every component's
  operations and every reaction once.
- `request`: a request names its node by key address; the node's kind
  names its handler (`HandleRequest`), which gets an edit handle
  (`Edit`) that adds, ends, replaces and moves nodes in the node's own
  subtree and lends the handler's needs at the node.
- `driver`: `Driver::new` builds the root; the driver is a future that
  handles what was woken, the highest priority first, until the root
  ended. After each lending it handles the requests put in, in order,
  before anything else runs.
  `Driver::operate` lends at a node by key address from outside.
  Dropping the driver ends every node, the nodes below first; after a
  panic through it, no component's operation runs again.
- `id`: the type-level numbers of components (`#[derive(Numbered)]`), by
  which needs are checked.

The crate re-exports `frunk_core`, whose lists the structure, the needs and
the views are made of.
