// expect: type mismatch resolving
use nitori_call::{ReceiverFamily,DirectView};
struct Wrong;
impl ReceiverFamily for Wrong {type ReceiverView<'v> = DirectView<'v,()>;}
fn main() {}
