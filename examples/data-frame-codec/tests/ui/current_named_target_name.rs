// expect: pass
#![feature(coroutines,coroutine_trait,type_alias_impl_trait)]
use nitori_call::{TargetExt as _, PollCallExt as _};
use nitori_call::call;
use nitori_call::CallOn;
use nitori_data_frame_codec_example::current_codec::TargetReadVarintExt;
#[call]
pub async fn twice<Transport: Read>(io: nitori_call::Target<Transport>) -> Result<u64,ReadBeError<Transport::Error>> {
    Ok(io.read_varint().await? + io.read_varint().await?)
}
fn check<T:Read>() {fn call_on<T:nitori_call::ReceiverFamily,C:CallOn<T>>(){} call_on::<T,Twice<T>>();}
use nitori_io::{Read, calls::ReadBeError};
fn main(){}
