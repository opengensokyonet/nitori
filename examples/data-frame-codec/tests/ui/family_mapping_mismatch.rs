// expect: type mismatch resolving
use nitori_call::{HostFamily,DirectView};
struct Wrong;
impl HostFamily for Wrong {type Host<'v> = DirectView<'v,()>;}
fn main() {}
