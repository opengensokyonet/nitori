// expect: in arguments and where clauses, name an associated type of the family with its trait
// facade-only
#![feature(impl_trait_in_assoc_type)]
use nitori_call::{ReceiverAccess, call};
use nitori_io::ReadChunk;
#[call]
async fn shorthand<F: ReadChunk, A: ReceiverAccess<Family = F>>(_io: &mut A, _chunk: F::Chunk) {}
fn main() {}
