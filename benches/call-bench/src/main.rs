//! Framed reads from a tokio duplex pipe fed by a writer task, with ordinary
//! async and with `nitori_call`.
//!
//! Each run creates a fresh pipe with a 64 KiB buffer. In bulk mode the
//! writer sends 1460-byte segments as fast as the buffer allows; in trickle
//! mode it writes one frame and yields to the scheduler, so the reader waits
//! for nearly every frame. Run with `--release`; pass `trickle` for trickle
//! mode or `sizes` to print the sizes of the per-frame futures.
#![feature(impl_trait_in_assoc_type, yield_expr)]
#![allow(clippy::multiple_bound_locations)]
mod call;
mod common;
mod plain;

use std::{
    cell::Cell,
    future::{Future, poll_fn},
    pin::pin,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncWriteExt, BufReader},
    runtime::Runtime,
    sync::Mutex,
};

#[derive(Clone, Copy, Debug)]
enum Variant {
    PlainExclusive,
    CallExclusive,
    CallExclusiveStream,
    PlainHoldPerFrame,
    PlainLockPerPoll,
    CallShared,
    CallSharedStream,
}
use Variant::*;
const VARIANTS: [Variant; 7] = [
    PlainExclusive,
    CallExclusive,
    CallExclusiveStream,
    PlainHoldPerFrame,
    PlainLockPerPoll,
    CallShared,
    CallSharedStream,
];

async fn counted<F: Future>(polls: &Cell<usize>, future: F) -> F::Output {
    let mut future = pin!(future);
    poll_fn(|cx| {
        polls.set(polls.get() + 1);
        future.as_mut().poll(cx)
    })
    .await
}

#[derive(Clone)]
struct Input {
    bytes: Arc<Vec<u8>>,
    ends: Arc<Vec<usize>>,
    checksum: u64,
    trickle: bool,
}
impl Input {
    fn new(frames: usize, trickle: bool) -> Self {
        let (bytes, ends, checksum) = common::encode(frames);
        Self {
            bytes: Arc::new(bytes),
            ends: Arc::new(ends),
            checksum,
            trickle,
        }
    }
    fn frames(&self) -> usize {
        self.ends.len()
    }
}

struct Sample {
    elapsed: Duration,
    locks: usize,
    polls: usize,
}

/// Read every frame of `input` with `variant`, checking the checksum.
fn run_once(runtime: &Runtime, variant: Variant, input: Input) -> Sample {
    let frames = input.frames();
    let expected = input.checksum;
    runtime.block_on(async move {
        let (mut client, server) = tokio::io::duplex(64 * 1024);
        let start = Instant::now();
        // Joined below; it ends after writing every frame.
        let writer = tokio::spawn(async move {
            if input.trickle {
                let mut begin = 0;
                for &end in input.ends.iter() {
                    client.write_all(&input.bytes[begin..end]).await.unwrap();
                    begin = end;
                    tokio::task::yield_now().await;
                }
            } else {
                for segment in input.bytes.chunks(1460) {
                    client.write_all(segment).await.unwrap();
                }
            }
        });
        let source = BufReader::new(server);
        let locks = Cell::new(0);
        let polls = Cell::new(0);
        let shared = Mutex::new(source);
        let checksum = match variant {
            PlainExclusive => counted(&polls, plain::exclusive(shared.into_inner(), frames)).await,
            CallExclusive => counted(&polls, call::exclusive(shared.into_inner(), frames)).await,
            CallExclusiveStream => {
                counted(&polls, call::exclusive_stream(shared.into_inner(), frames)).await
            }
            PlainHoldPerFrame => {
                counted(&polls, plain::hold_per_frame(&shared, &locks, frames)).await
            }
            PlainLockPerPoll => {
                counted(&polls, plain::lock_per_poll(&shared, &locks, frames)).await
            }
            CallShared => counted(&polls, call::shared(&shared, &locks, frames)).await,
            CallSharedStream => counted(&polls, call::shared_stream(&shared, &locks, frames)).await,
        };
        let elapsed = start.elapsed();
        writer.await.unwrap();
        assert_eq!(checksum, expected, "{variant:?}");
        Sample {
            elapsed,
            locks: locks.get(),
            polls: polls.get(),
        }
    })
}

fn runtimes() -> [(&'static str, Runtime); 2] {
    [
        (
            "current_thread",
            tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap(),
        ),
        (
            "multi_thread(2), writer on a worker",
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .build()
                .unwrap(),
        ),
    ]
}

fn sizes() {
    use std::mem::size_of_val;
    let (_, server) = tokio::io::duplex(1);
    let mut source = BufReader::new(server);
    let mut payload = Vec::new();
    let locks = Cell::new(0);
    println!(
        "plain read_frame: {} bytes",
        size_of_val(&plain::read_frame(&mut source, &mut payload))
    );
    let mut call_conn = call::Conn(source);
    {
        #[allow(unused_imports)]
        use call::*;
        println!(
            "nitori_call read_frame on a receiver: {} bytes",
            size_of_val(&call_conn.read_frame_unpin(&mut payload))
        );
    }
    let mutex = Mutex::new(call_conn.0);
    {
        #[allow(unused_imports)]
        use call::*;
        let mut reader = call::Reader {
            shared: &mutex,
            locks: &locks,
        };
        println!(
            "nitori_call read_frame on the lock executor: {} bytes",
            size_of_val(&reader.read_frame_unpin(&mut payload))
        );
    }
}

fn main() {
    if std::env::args().any(|argument| argument == "sizes") {
        return sizes();
    }
    let trickle = std::env::args().any(|argument| argument == "trickle");
    let input = Input::new(if trickle { 50_000 } else { 200_000 }, trickle);
    let frames = input.frames();
    let repeats = 9;
    println!(
        "{} mode: {frames} frames, {} bytes, {repeats} runs each, median (min–max)",
        if trickle { "trickle" } else { "bulk" },
        input.bytes.len()
    );
    for (name, runtime) in &runtimes() {
        println!("\n== {name}");
        for variant in VARIANTS {
            run_once(runtime, variant, input.clone());
        }
        let mut samples: Vec<Vec<Sample>> = VARIANTS.iter().map(|_| Vec::new()).collect();
        for _ in 0..repeats {
            for (index, variant) in VARIANTS.iter().enumerate() {
                samples[index].push(run_once(runtime, *variant, input.clone()));
            }
        }
        for (variant, samples) in VARIANTS.iter().zip(&mut samples) {
            samples.sort_by_key(|sample| sample.elapsed);
            let per_frame = |sample: &Sample| sample.elapsed.as_nanos() as f64 / frames as f64;
            let median = &samples[samples.len() / 2];
            println!(
                "{:<26} {:7.1} ns/frame ({:6.1}–{:6.1})  {:6.3} polls/frame  {:6.3} locks/frame",
                format!("{variant:?}"),
                per_frame(median),
                per_frame(&samples[0]),
                per_frame(samples.last().unwrap()),
                median.polls as f64 / frames as f64,
                median.locks as f64 / frames as f64,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_variant_reads_every_frame() {
        for trickle in [false, true] {
            let input = Input::new(500, trickle);
            for (_, runtime) in &runtimes() {
                for variant in VARIANTS {
                    // `run_once` checks the checksum.
                    run_once(runtime, variant, input.clone());
                }
            }
        }
    }
}
