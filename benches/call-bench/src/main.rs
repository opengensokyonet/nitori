//! HTTP responses read and written with ordinary async, tokio-util codecs and
//! `nitori_call`, each written the way that suits it best. The responses come
//! from a captured corpus (see [`workload`]); field sections use HPACK or
//! QPACK with the static table only (see [`fields`]).
//!
//! Scenarios:
//! - `h2-read`, `h2-write`: HTTP/2 over a tokio duplex pipe whose every read
//!   or write has a fixed cost, like a TCP socket; see [`h2`];
//! - `h3-recv`, `h3-send`: HTTP/3 over the stream buffers of a QUIC
//!   connection in user space, one task per stream; see [`h3`];
//! - `fields`: the cost of encoding and decoding the field sections alone.
//!
//! Shared resources sit behind a fair async lock (`tokio::sync::Mutex`), an
//! unfair async lock (`futures_util::lock::Mutex`) or a blocking lock
//! (`std::sync::Mutex`).
//!
//! Run with `--release`, a scenario and optionally:
//! - `streams=N`: responses in flight (HTTP/2) or streams (HTTP/3), default 8;
//! - `chunk=N`: the most body bytes per DATA frame, default 16384;
//! - `passes=N`: copies of the corpus, default 8 (2 with `trickle`);
//! - `trickle`: yield to the scheduler after each segment or packet, so
//!   readers wait for nearly every one;
//! - `contention` (`h2-read`): a probe task locks the shared pipe every
//!   20 µs; report how long it waited.
#![feature(impl_trait_in_assoc_type)]
#![allow(clippy::multiple_bound_locations)]
mod calls;
mod codec;
mod common;
mod fields;
mod h2;
mod h3;
mod tables;
mod workload;

use common::{LOCK_KINDS, LockKind, Sample};
use std::time::{Duration, Instant};
use tokio::runtime::Runtime;

fn runtimes() -> [(&'static str, Runtime); 2] {
    [
        (
            "current_thread",
            tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap(),
        ),
        (
            "multi_thread(2)",
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .build()
                .unwrap(),
        ),
    ]
}

fn lock_name(kind: LockKind) -> &'static str {
    match kind {
        LockKind::Fair => "tokio::sync::Mutex (fair)",
        LockKind::Unfair => "futures_util::lock::Mutex (unfair)",
        LockKind::Blocking => "std::sync::Mutex (blocking)",
    }
}

fn percentile(sorted: &[Duration], fraction: f64) -> Duration {
    sorted[((sorted.len() - 1) as f64 * fraction).round() as usize]
}

/// Rounds after one warm-up run of each variant.
const REPEATS: usize = 9;

/// Run every variant, then print the median per variant.
fn report<V: Copy + std::fmt::Debug>(
    variants: &[V],
    frames: usize,
    bytes: usize,
    mut run: impl FnMut(V) -> Sample,
) {
    for &variant in variants {
        run(variant);
    }
    let mut samples: Vec<Vec<Sample>> = variants.iter().map(|_| Vec::new()).collect();
    for _ in 0..REPEATS {
        for (index, &variant) in variants.iter().enumerate() {
            samples[index].push(run(variant));
        }
    }
    let frames = frames as f64;
    for (variant, samples) in variants.iter().zip(&mut samples) {
        samples.sort_by_key(|sample| sample.elapsed);
        let per_frame = |sample: &Sample| sample.elapsed.as_nanos() as f64 / frames;
        let median = &samples[samples.len() / 2];
        print!(
            "  {:<18} {:8.1} ns/frame ({:7.1}–{:7.1}) {:5.2} GB/s {:6.3} polls/frame {:6.3} locks/frame",
            format!("{variant:?}"),
            per_frame(median),
            per_frame(&samples[0]),
            per_frame(samples.last().unwrap()),
            bytes as f64 / median.elapsed.as_nanos() as f64,
            median.polls as f64 / frames,
            median.locks as f64 / frames,
        );
        if median.buffer > 0 {
            print!("  buffer {} B", median.buffer);
        }
        if !median.waits.is_empty() {
            let mut waits = median.waits.clone();
            waits.sort();
            print!(
                "  other side: {} locks, wait p50 {:?} p99 {:?} max {:?}",
                waits.len(),
                percentile(&waits, 0.5),
                percentile(&waits, 0.99),
                waits.last().unwrap(),
            );
        }
        println!();
    }
}

/// Encode and decode every field section on its own.
fn field_costs(messages: &[workload::Message]) {
    use fields::{Fields, hpack, qpack};
    let raw: usize = messages
        .iter()
        .flat_map(|m| &m.fields)
        .map(|(n, v)| n.len() + v.len())
        .sum();
    println!(
        "{} field sections, {:.0} bytes of names and values each on average",
        messages.len(),
        raw as f64 / messages.len() as f64
    );
    type Encode = fn(&workload::Message, &mut Vec<u8>);
    type Decode = fn(&[u8], &mut Fields) -> Result<(), fields::FieldError>;
    let codecs: [(&str, Encode, Decode); 2] = [
        (
            "HPACK",
            |m, out| hpack::encode(m.fields.iter().map(|(n, v)| (n.as_str(), v.as_str())), out),
            hpack::decode,
        ),
        (
            "QPACK",
            |m, out| qpack::encode(m.fields.iter().map(|(n, v)| (n.as_str(), v.as_str())), out),
            qpack::decode,
        ),
    ];
    for (name, encode, decode) in codecs {
        let mut encoded: Vec<Vec<u8>> = Vec::new();
        let mut best_encode = Duration::MAX;
        let mut best_decode = Duration::MAX;
        let mut fields = Fields::default();
        for _ in 0..REPEATS {
            let start = Instant::now();
            encoded = messages
                .iter()
                .map(|message| {
                    let mut output = Vec::new();
                    encode(message, &mut output);
                    output
                })
                .collect();
            best_encode = best_encode.min(start.elapsed());
            let start = Instant::now();
            let mut checksum = 0;
            for block in &encoded {
                decode(block, &mut fields).unwrap();
                checksum += fields.checksum();
            }
            best_decode = best_decode.min(start.elapsed());
            std::hint::black_box(checksum);
        }
        let size: usize = encoded.iter().map(Vec::len).sum();
        let count = messages.len() as f64;
        println!(
            "  {name}: {:.0} bytes encoded on average ({:.0}% of the raw size); encode {:.0} ns, decode {:.0} ns per section (best of {REPEATS})",
            size as f64 / count,
            100.0 * size as f64 / raw as f64,
            best_encode.as_nanos() as f64 / count,
            best_decode.as_nanos() as f64 / count,
        );
    }
}

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let has = |name: &str| arguments.iter().any(|argument| argument == name);
    let value = |name: &str, default: usize| {
        arguments
            .iter()
            .find_map(|argument| argument.strip_prefix(&format!("{name}=")))
            .map_or(default, |value| value.parse().expect("a number"))
    };
    let scenario = arguments.first().map(String::as_str).unwrap_or("h3-recv");
    let trickle = has("trickle");
    let streams = value("streams", 8);
    let chunk = value("chunk", 16 * 1024);
    let passes = value("passes", if trickle { 2 } else { 8 });
    let messages = workload::messages(passes);
    if scenario == "fields" {
        return field_costs(&messages[..messages.len() / passes]);
    }
    let frames: usize = messages
        .iter()
        .map(|m| 1 + m.body.len().div_ceil(chunk))
        .sum();
    let bytes: usize = messages.iter().map(|m| m.body.len()).sum();
    println!(
        "{scenario}: {} responses, {frames} frames, {bytes} body bytes, {streams} streams, chunk {chunk}{}, {REPEATS} runs each, median (min–max)",
        messages.len(),
        if trickle { ", trickle" } else { "" },
    );
    for (name, runtime) in &runtimes() {
        println!("\n== {name}");
        match scenario {
            "h2-read" => {
                let input = h2::Input::new(messages.clone(), streams, chunk, trickle);
                let contention = has("contention");
                if !contention {
                    println!("exclusive");
                    report(&h2::READS, frames, bytes, |variant| {
                        h2::run_read(runtime, variant, input.clone())
                    });
                }
                for lock in LOCK_KINDS {
                    println!("shared, {}", lock_name(lock));
                    let variants: Vec<_> = h2::SHARED_READS
                        .into_iter()
                        .filter(|variant| variant.supports(lock))
                        .collect();
                    report(&variants, frames, bytes, |variant| {
                        h2::run_shared_read(runtime, variant, lock, input.clone(), contention)
                    });
                }
            }
            "h2-write" => {
                let input = h2::Input::new(messages.clone(), streams, chunk, trickle);
                report(&h2::WRITES, frames, bytes, |variant| {
                    h2::run_write(runtime, variant, input.clone())
                });
            }
            "h3-recv" => {
                let input = h3::Input::new(messages.clone(), streams, chunk, trickle);
                for lock in LOCK_KINDS {
                    println!("{}", lock_name(lock));
                    report(&h3::RECVS, frames, bytes, |variant| {
                        h3::run_recv(runtime, variant, lock, input.clone())
                    });
                }
            }
            "h3-send" => {
                let input = h3::Input::new(messages.clone(), streams, chunk, trickle);
                for lock in LOCK_KINDS {
                    println!("{}", lock_name(lock));
                    report(&h3::SENDS, frames, bytes, |variant| {
                        h3::run_send(runtime, variant, lock, input.clone())
                    });
                }
            }
            other => panic!(
                "unknown scenario {other}; use h2-read, h2-write, h3-recv, h3-send or fields"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Each run checks the checksum of what was read or written.

    fn inputs() -> impl Iterator<Item = (usize, usize, bool)> {
        [(1, 16 * 1024, false), (4, 4096, true), (3, 1000, false)].into_iter()
    }

    #[test]
    fn h2_variants_carry_every_frame() {
        let messages = workload::messages(1);
        for (streams, chunk, trickle) in inputs() {
            let input = h2::Input::new(messages.clone(), streams, chunk, trickle);
            for (_, runtime) in &runtimes() {
                for variant in h2::READS {
                    h2::run_read(runtime, variant, input.clone());
                }
                for variant in h2::WRITES {
                    h2::run_write(runtime, variant, input.clone());
                }
                for lock in LOCK_KINDS {
                    for variant in h2::SHARED_READS.into_iter().filter(|v| v.supports(lock)) {
                        let sample =
                            h2::run_shared_read(runtime, variant, lock, input.clone(), true);
                        assert!(!sample.waits.is_empty(), "{variant:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn h3_variants_carry_every_frame() {
        let messages = workload::messages(1);
        for (streams, chunk, trickle) in inputs() {
            let input = h3::Input::new(messages.clone(), streams, chunk, trickle);
            for (_, runtime) in &runtimes() {
                for lock in LOCK_KINDS {
                    for variant in h3::RECVS {
                        h3::run_recv(runtime, variant, lock, input.clone());
                    }
                    for variant in h3::SENDS {
                        h3::run_send(runtime, variant, lock, input.clone());
                    }
                }
            }
        }
    }
}
