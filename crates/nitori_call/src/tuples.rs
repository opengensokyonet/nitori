//! The names the impls on tuples share, so that they all stop at one size.

/// Invoke a macro for each tuple size from 2 to 12, with a row of names for
/// each element: three type parameters, a local, the field and two lifetimes.
macro_rules! for_tuples {
    ($apply:ident) => {
        for_tuples!(@grow $apply
            [(A0 B0 C0 a0 0 'a0 'h0) (A1 B1 C1 a1 1 'a1 'h1)]
            (A2 B2 C2 a2 2 'a2 'h2)
            (A3 B3 C3 a3 3 'a3 'h3)
            (A4 B4 C4 a4 4 'a4 'h4)
            (A5 B5 C5 a5 5 'a5 'h5)
            (A6 B6 C6 a6 6 'a6 'h6)
            (A7 B7 C7 a7 7 'a7 'h7)
            (A8 B8 C8 a8 8 'a8 'h8)
            (A9 B9 C9 a9 9 'a9 'h9)
            (A10 B10 C10 a10 10 'a10 'h10)
            (A11 B11 C11 a11 11 'a11 'h11));
    };
    (@grow $apply:ident [$($row:tt)+] $next:tt $($rest:tt)*) => {
        $apply!($($row)+);
        for_tuples!(@grow $apply [$($row)+ $next] $($rest)*);
    };
    (@grow $apply:ident [$($row:tt)+]) => {
        $apply!($($row)+);
    };
}
