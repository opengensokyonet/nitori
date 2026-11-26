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
  handles. A kind is at one place of the structure only. Each part lists
  a share (`Share`) for each kind of node it extends, in `Part::Shares`:
  components, the calls that run there, and its reactions; in the other
  kinds it keeps nothing. Key addresses follow from the kinds alone.
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
- `component`: a `Build` runs when the node is added. An ordinary
  component list builds each component in order. A share can also list
  `Constructors<HList![One<A>, Group<B>]>` to run single and grouped
  builds. A group returns a list whose members enter the share as
  independent components. Each `Component` declares its `wake` and
  `end` needs. A constructor gets its part's arguments and can find
  another part's with `part_args`.
- `capability` and `react`: capabilities put together from needs, or
  contributed by the parts, each part's contribution to a family listed
  in the part or in the composition; reaction points that the other parts react
  to where an action happens, among them the driver's `NodeStarted` and
  `NodeEnding`.
- `call`: kinds of calls (`CallKind`, written with `nitori_call`'s
  `#[call]`), lent their needs on every poll; a call sets the priority its
  standard waker wakes it at, and `settle`s to wait for the requests it
  put in. A composition (`composition!`) names a root kind, the parts, a
  clock, a priority type and, when it has one, its own list of
  contributions, and proves every call, every component's
  operations and every reaction once; when one of them does not compile,
  `explain!` proves it on its own, and its error says which need fails
  and why. Use the `build` form for a constructor's needs.
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
- `id`: the type-level numbers (`#[derive(Numbered)]`) of components,
  node kinds, parts and families of contributed capabilities, by which
  needs are checked and shares, contributions and arguments found; the
  proof of a composition checks that every part has one.

The crate re-exports `frunk_core`, whose lists the structure, the needs and
the views are made of.
