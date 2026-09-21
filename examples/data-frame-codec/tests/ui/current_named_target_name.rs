// expect: pass
#![feature(coroutines,coroutine_trait,type_alias_impl_trait)]
use nitori_call::{ReceiverExt as _, PollCallExt as _};
use nitori_call::call;
use nitori_call::CallOn;
use nitori_data_frame_codec_example::{current_codec::ReceiverReadVarintExt,current_codec::{ReadSource,CodecError}};
#[call]
pub async fn twice<Transport: ReadSource>(io: nitori_call::Receiver<Transport>) -> Result<u64,CodecError> {
    Ok(io.read_varint().await? + io.read_varint().await?)
}
fn check<T:ReadSource>() {fn call_on<T:nitori_call::HostFamily,C:CallOn<T>>(){} call_on::<T,Twice<T>>();}
fn main(){}
