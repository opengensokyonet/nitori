# nitori_driver_timer

The timer component of `nitori_driver`, and the part that keeps it at the
root. It is experimental, builds without the standard library with
`alloc`, and needs the nightly toolchain the repository pins.

A component that is to be woken at an instant sets it in `TimerTable` with
its waker and a priority, `set(at, &waker, p)`; one waker has one instant,
setting it again replaces it, and an instant wakes once. `clear(&waker)`
forgets it. A call at the root sleeps on the composition's clock until the
earliest instant and wakes the components that are due at the priorities
they were set with; the driver itself knows nothing of timers.

`nitori_driver_timer::Part<Q>` keeps the table and that call at the root of
the composition `Q`, with its clock and priority, and is written in `Q`'s
own list of parts:

```rust,ignore
composition!(impl for Conn: ConnRoot, HList![v1::Part, nitori_driver_timer::Part<Conn>], SysClock, ConnPriority);
```

The part has a type-level number, the same whatever `Q` is, so that the
composition can have its own list of contributions and its components can
find other parts' arguments with `part_args`; `Q` needs no number.

It is a package of its own, not part of `nitori_clock`: `nitori_driver`
depends on `nitori_clock`, and the table is a `nitori_driver` component.
The default `std` feature turns on `nitori_driver`'s, so that a driver
with the part can be `Send`.
