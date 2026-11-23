# nitori_driver

The tree of state a driver keeps and lends to its calls. It is experimental
and builds without the standard library, with `alloc`; it needs the nightly
toolchain the repository pins.

A driver holds the whole state of one connection, or of another protocol's
connection, and advances many calls in turn. This crate gives the mechanism
without protocol meaning; the code that uses it declares its own structure,
parts and calls. It is being rebuilt: the tree and the lending of needs are
here, the driver around them is not yet.

- `structure`: node kinds declare their children (a kind and a storage
  kind each); a node holds the parts' shares and the stores of its
  children. Typed paths and the driver's addresses follow from the kinds
  alone, so they name no part; a kind is at one place of the structure
  only. A kind declares what a new node of it is given to start from and
  why it is asked to end.
- `need`: what is taken from the tree around a lend point, written in
  terms of components: on the way up to the root (`Mut`, `Ref`), at every
  node of one kind below an anchor (`Under`), from another lend point
  (`At`), and the lend point's own key and address (`OwnKey`,
  `OwnAddress`). The view of a need depends on the need and on a position
  inferred from the structure, so a call generic over the structure names
  its view from its needs; how many members a result has follows from the
  structure, and every shape gives the same member-by-member access.
- `id`: the type-level numbers of components (`#[derive(Numbered)]`),
  by which a list of needs is checked where it is proved: no node or more
  than one node having a component, and two needs that overlap, do not
  compile, and the error names the component and the count.

The crate re-exports `frunk_core`, whose lists the structure, the needs and
the views are made of.
