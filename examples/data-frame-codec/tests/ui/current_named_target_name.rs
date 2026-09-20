// expect: pass
#![feature(coroutines,coroutine_trait,type_alias_impl_trait)]
use nitori_call::call;
use nitori_call::CallOn;
use nitori_data_frame_codec_example::{current_codec::ReadVarint,current_codec::{ReadSource,CodecError}};
#[call]
pub async fn twice<Transport: ReadSource + ?Sized>(io: std::pin::Pin<&mut Transport>) -> Result<u64,CodecError> {
    Ok(io.read_varint().await? + io.read_varint().await?)
}
fn check<T:ReadSource + ?Sized>() {fn call_on<T:?Sized,C:CallOn<T>>(){} call_on::<T,Twice<T>>();}
fn main(){}
