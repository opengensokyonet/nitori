// Returns a real host-free child through a normal function/closure boundary.
#[call]
async fn escaped(io:Receiver<'_,Host>)->nitori_call::typed::Child<Host,Decode>{
 let value: nitori_call::typed::Child<Host, Decode> = io.decode(8); value
}
#[call]
async fn reentered(io:Receiver<'_,Host>)->usize{
 let value=io.escaped().await;
 let factory=move || value;
 factory().await
}
