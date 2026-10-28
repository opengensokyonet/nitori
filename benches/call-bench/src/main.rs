//! HTTP responses read and written with ordinary async, tokio-util codecs and
//! `nitori_call`, each written the way that suits it best. The responses come
//! from a captured corpus (see [`workload`]); field sections use HPACK or
//! QPACK with the static table only (see [`fields`]).
//!
//! Scenarios:
//! - `h2-read`, `h2-write`: HTTP/2 over a tokio duplex pipe whose every read
//!   or write has a fixed cost, like a TCP socket; see [`h2`];
//! - `h3-recv`, `h3-send`: HTTP/3 over the stream buffers of a QUIC
//!   connection in user space, each behind a lock of its own, one task per
//!   stream; see [`h3`];
//! - `qpack-encoder-recv`, `qpack-encoder-send`: QPACK encoder stream
//!   instructions over the same stream buffers, one encoder stream per
//!   connection; see [`encoder_stream`];
//! - `fields`: the cost of encoding and decoding the field sections alone;
//! - `mechanism`: the fixed cost of an access through each way, with no work
//!   behind it; see [`mechanism`].
//!
//! Shared resources sit behind a fair async lock (`tokio::sync::Mutex`), an
//! unfair async lock (`futures_util::lock::Mutex`) or a blocking lock
//! (`std::sync::Mutex`). Every variant does as much as it can with each
//! acquisition, until nothing more is ready. For each side of a lock the
//! report counts acquisitions, those that found it held by someone else
//! (queued), and those after which the value had not changed (empty).
//!
//! Run with `--release`, a scenario and optionally:
//! - `streams=N`: responses in flight (HTTP/2), streams (HTTP/3) or
//!   connections (`qpack-encoder-*`), default 8;
//! - `chunk=N`: the most body bytes per DATA frame, default 16384;
//! - `passes=N`: copies of the corpus, default 8 (2 with `trickle`);
//! - `rate=N`: the source's bytes arrive at N MB/s (10^6 bytes), released
//!   each millisecond; by default as fast as they are taken (`h2-read`,
//!   `h3-recv`, `h3-send`, `qpack-encoder-*`);
//! - `trickle`: yield to the scheduler after each segment or packet received,
//!   so readers wait for nearly every one;
//! - `update=N` (`h3-*`, `qpack-encoder-*`): bytes a reader consumes before
//!   it raises its stream's flow control limit, and the most room a waiting
//!   writer asks for; default 32768, half the window;
//! - `contention[=N]` (`h2-read`): a control task locks the shared reader
//!   once for every N bytes fed, default 32768; report how long it waited;
//! - `capacity=N` (`qpack-encoder-*`): the dynamic table capacity, default
//!   4096;
//! - `work=N`: passes the application makes over every DATA byte after
//!   reading it or before writing it, default 1; with 0 it only counts the
//!   bytes;
//! - `holds`: also measure how long each side holds its locks, which adds
//!   two clock reads to every acquisition.
#![feature(impl_trait_in_assoc_type, yield_expr)]
#![allow(clippy::multiple_bound_locations)]
mod calls;
mod codec;
mod common;
mod encoder_stream;
mod fields;
mod h2;
mod h3;
mod mechanism;
mod tables;
mod workload;

use common::{LOCK_KINDS, LockCounts, LockKind, Pace, Sample};
use std::time::{Duration, Instant};
use tokio::runtime::Runtime;
use workload::Work;

fn runtimes() -> [(&'static str, Runtime); 2] {
    [
        (
            "current_thread",
            tokio::runtime::Builder::new_current_thread()
                .enable_time()
                .build()
                .unwrap(),
        ),
        (
            "multi_thread(2)",
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_time()
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

/// Acquisitions per unit of work, and the shares queued and, with `empty`,
/// empty. The other side's acquisitions always deliver, take or do nothing,
/// so its empty share says nothing.
fn locks(counts: &LockCounts, units: f64, unit: &str, empty: bool) -> String {
    let share = |count: usize| 100.0 * count as f64 / counts.acquired.max(1) as f64;
    let mut text = format!(
        "{:6.3} locks/{unit} ({:4.1}% queued",
        counts.acquired as f64 / units,
        share(counts.queued),
    );
    if empty {
        text += &format!(", {:4.1}% empty", share(counts.empty));
    }
    if let Some(held) = counts.held {
        let mean = held.as_nanos() as f64 / counts.acquired.max(1) as f64;
        text += &format!(", held {mean:.0} ns each");
    }
    text + ")"
}

fn percentile(sorted: &[Duration], fraction: f64) -> Duration {
    sorted[((sorted.len() - 1) as f64 * fraction).round() as usize]
}

/// Rounds after one warm-up run of each variant.
const REPEATS: usize = 9;

/// Run every variant, then print the median per variant, per unit of work:
/// a frame, or an instruction.
fn report<V: Copy + std::fmt::Debug>(
    variants: &[V],
    (units, unit): (usize, &str),
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
    let units = units as f64;
    for (variant, samples) in variants.iter().zip(&mut samples) {
        samples.sort_by_key(|sample| sample.elapsed);
        let per_unit = |sample: &Sample| sample.elapsed.as_nanos() as f64 / units;
        let median = &samples[samples.len() / 2];
        print!(
            "  {:<20} {:8.1} ns/{unit} ({:7.1}–{:7.1}) {:5.2} GB/s {:6.3} polls/{unit}",
            format!("{variant:?}"),
            per_unit(median),
            per_unit(&samples[0]),
            per_unit(samples.last().unwrap()),
            bytes as f64 / median.elapsed.as_nanos() as f64,
            median.polls as f64 / units,
        );
        if median.locks.acquired > 0 {
            print!(" {}", locks(&median.locks, units, unit, true));
        }
        if median.buffer > 0 {
            print!("  buffer {} B", median.buffer);
        }
        if !median.waits.is_empty() {
            let mut waits = median.waits.clone();
            waits.sort();
            print!(
                "  other side: {}, wait p50 {:?} p99 {:?} max {:?}",
                locks(&median.other, units, unit, false),
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

/// Receive or send the QPACK encoder stream of each connection.
fn qpack_encoder(send: bool, input: encoder_stream::Input, holds: bool) {
    let mut options = String::new();
    if let Pace::Rate(rate) = input.pace {
        options += &format!(", {} MB/s", rate / 1_000_000);
    }
    if input.trickle {
        options += ", trickle";
    }
    let scenario = if send {
        "qpack-encoder-send"
    } else {
        "qpack-encoder-recv"
    };
    println!(
        "{scenario}: {} instructions, {} bytes, {} connections, capacity {}, update {}{options}, {REPEATS} runs each, median (min–max)",
        input.instructions,
        input.size(),
        input.streams,
        input.capacity,
        input.update,
    );
    if send {
        let mut best = Duration::MAX;
        for _ in 0..REPEATS {
            let start = Instant::now();
            for slot in 0..input.streams {
                let sections = input.messages.iter().skip(slot).step_by(input.streams);
                std::hint::black_box(encoder_stream::encode(sections, input.capacity));
            }
            best = best.min(start.elapsed());
        }
        println!(
            "choosing and encoding the instructions alone, into one buffer: {:.1} ns/instruction (best of {REPEATS})",
            best.as_nanos() as f64 / input.instructions as f64,
        );
    }
    for (name, runtime) in &runtimes() {
        println!("\n== {name}");
        for lock in LOCK_KINDS {
            println!("{}", lock_name(lock));
            let units = (input.instructions, "instruction");
            if send {
                report(&encoder_stream::SENDS, units, input.size(), |variant| {
                    encoder_stream::run_send(runtime, variant, lock, input.clone(), holds)
                });
            } else {
                report(&encoder_stream::RECVS, units, input.size(), |variant| {
                    encoder_stream::run_recv(runtime, variant, lock, input.clone(), holds)
                });
            }
        }
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
    let holds = has("holds");
    let streams = value("streams", 8);
    let chunk = value("chunk", 16 * 1024);
    let passes = value("passes", if trickle { 2 } else { 8 });
    let update = value("update", h3::WINDOW / 2);
    let capacity = value("capacity", 4096);
    let work = Work {
        passes: value("work", 1),
    };
    let pace = match value("rate", 0) {
        0 => Pace::Unlimited,
        rate => Pace::Rate(rate as u64 * 1_000_000),
    };
    let contention = (has("contention") || arguments.iter().any(|a| a.starts_with("contention=")))
        .then(|| value("contention", 32 * 1024));
    let messages = workload::messages(passes);
    if scenario == "mechanism" {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        return mechanism::run(&runtime);
    }
    if scenario == "fields" {
        return field_costs(&messages[..messages.len() / passes]);
    }
    if let Some(side) = scenario.strip_prefix("qpack-encoder-") {
        let send = match side {
            "recv" => false,
            "send" => true,
            other => panic!("unknown side {other}; use qpack-encoder-recv or qpack-encoder-send"),
        };
        let input = encoder_stream::Input::new(messages, streams, pace, trickle, update, capacity);
        return qpack_encoder(send, input, holds);
    }
    let frames: usize = messages
        .iter()
        .map(|m| 1 + m.body.len().div_ceil(chunk))
        .sum();
    let bytes: usize = messages.iter().map(|m| m.body.len()).sum();
    let mut options = String::new();
    if let Pace::Rate(rate) = pace {
        options += &format!(", {} MB/s", rate / 1_000_000);
    }
    if trickle {
        options += ", trickle";
    }
    if work.passes != 1 {
        options += &format!(", work {}", work.passes);
    }
    if scenario.starts_with("h3") {
        options += &format!(", update {update}");
    }
    if let Some(every) = contention {
        options += &format!(", control lock every {every} bytes");
    }
    println!(
        "{scenario}: {} responses, {frames} frames, {bytes} body bytes, {streams} streams, chunk {chunk}{options}, {REPEATS} runs each, median (min–max)",
        messages.len(),
    );
    for (name, runtime) in &runtimes() {
        println!("\n== {name}");
        match scenario {
            "h2-read" => {
                let input = h2::Input::new(messages.clone(), streams, chunk, pace, trickle, work);
                if contention.is_none() {
                    println!("exclusive");
                    report(&h2::READS, (frames, "frame"), bytes, |variant| {
                        h2::run_read(runtime, variant, input.clone())
                    });
                }
                for lock in LOCK_KINDS {
                    println!("shared, {}", lock_name(lock));
                    let variants: Vec<_> = h2::SHARED_READS
                        .into_iter()
                        .filter(|variant| variant.supports(lock))
                        .collect();
                    report(&variants, (frames, "frame"), bytes, |variant| {
                        h2::run_shared_read(
                            runtime,
                            variant,
                            lock,
                            input.clone(),
                            contention,
                            holds,
                        )
                    });
                }
            }
            "h2-write" => {
                let input = h2::Input::new(messages.clone(), streams, chunk, pace, trickle, work);
                report(&h2::WRITES, (frames, "frame"), bytes, |variant| {
                    h2::run_write(runtime, variant, input.clone())
                });
            }
            "h3-recv" => {
                let input = h3::Input::new(
                    messages.clone(),
                    streams,
                    chunk,
                    pace,
                    trickle,
                    update,
                    work,
                );
                for lock in LOCK_KINDS {
                    println!("{}", lock_name(lock));
                    report(&h3::RECVS, (frames, "frame"), bytes, |variant| {
                        h3::run_recv(runtime, variant, lock, input.clone(), holds)
                    });
                }
            }
            "h3-send" => {
                let input = h3::Input::new(
                    messages.clone(),
                    streams,
                    chunk,
                    pace,
                    trickle,
                    update,
                    work,
                );
                for lock in LOCK_KINDS {
                    println!("{}", lock_name(lock));
                    report(&h3::SENDS, (frames, "frame"), bytes, |variant| {
                        h3::run_send(runtime, variant, lock, input.clone(), holds)
                    });
                }
            }
            other => panic!(
                "unknown scenario {other}; use h2-read, h2-write, h3-recv, h3-send, qpack-encoder-recv, qpack-encoder-send, fields or mechanism"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Each run checks the checksum of what was read or written.

    /// Streams, chunk, pace, trickle, update threshold and work.
    fn inputs() -> impl Iterator<Item = (usize, usize, Pace, bool, usize, Work)> {
        [
            (1, 16 * 1024, Pace::Unlimited, false, 32 * 1024, Work::CHECK),
            (4, 4096, Pace::Unlimited, true, 1, Work { passes: 0 }),
            (
                3,
                1000,
                Pace::Rate(2_000_000_000),
                false,
                60_000,
                Work { passes: 3 },
            ),
        ]
        .into_iter()
    }

    #[test]
    fn h2_variants_carry_every_frame() {
        let messages = workload::messages(1);
        for (streams, chunk, pace, trickle, _, work) in inputs() {
            let input = h2::Input::new(messages.clone(), streams, chunk, pace, trickle, work);
            for (_, runtime) in &runtimes() {
                for variant in h2::READS {
                    h2::run_read(runtime, variant, input.clone());
                }
                for variant in h2::WRITES {
                    h2::run_write(runtime, variant, input.clone());
                }
                for lock in LOCK_KINDS {
                    for variant in h2::SHARED_READS.into_iter().filter(|v| v.supports(lock)) {
                        let sample = h2::run_shared_read(
                            runtime,
                            variant,
                            lock,
                            input.clone(),
                            Some(16 * 1024),
                            true,
                        );
                        assert!(!sample.waits.is_empty(), "{variant:?}");
                        assert_eq!(sample.other.acquired, sample.waits.len(), "{variant:?}");
                        assert!(sample.locks.held.is_some(), "{variant:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn release_points_give_the_view_back() {
        let messages = workload::messages(1);
        let frames: usize = messages
            .iter()
            .map(|m| 1 + m.body.len().div_ceil(4096))
            .sum();
        let input = h3::Input::new(
            messages.clone(),
            2,
            4096,
            Pace::Unlimited,
            false,
            32 * 1024,
            Work::CHECK,
        );
        for (_, runtime) in &runtimes() {
            for lock in LOCK_KINDS {
                // Reading releases before processing every frame, so each
                // frame needs an acquisition of its own; writing
                // releases before every field section.
                let read = h3::run_recv(
                    runtime,
                    h3::RecvVariant::CallRelease,
                    lock,
                    input.clone(),
                    false,
                );
                assert!(read.locks.acquired >= frames, "{lock:?}");
                let write = h3::run_send(
                    runtime,
                    h3::SendVariant::CallRelease,
                    lock,
                    input.clone(),
                    false,
                );
                assert!(write.locks.acquired >= messages.len(), "{lock:?}");
            }
        }
    }

    #[test]
    fn qpack_encoder_variants_apply_every_instruction() {
        let messages = workload::messages(1);
        for (streams, _, pace, trickle, update, _) in inputs() {
            for capacity in [4096, 256] {
                let input = encoder_stream::Input::new(
                    messages.clone(),
                    streams,
                    pace,
                    trickle,
                    update,
                    capacity,
                );
                for (_, runtime) in &runtimes() {
                    for lock in LOCK_KINDS {
                        for variant in encoder_stream::RECVS {
                            encoder_stream::run_recv(runtime, variant, lock, input.clone(), false);
                        }
                        for variant in encoder_stream::SENDS {
                            encoder_stream::run_send(runtime, variant, lock, input.clone(), false);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn h3_variants_carry_every_frame() {
        let messages = workload::messages(1);
        for (streams, chunk, pace, trickle, update, work) in inputs() {
            let input = h3::Input::new(
                messages.clone(),
                streams,
                chunk,
                pace,
                trickle,
                update,
                work,
            );
            for (_, runtime) in &runtimes() {
                for lock in LOCK_KINDS {
                    for variant in h3::RECVS {
                        h3::run_recv(runtime, variant, lock, input.clone(), false);
                    }
                    for variant in h3::SENDS {
                        h3::run_send(runtime, variant, lock, input.clone(), false);
                    }
                }
            }
        }
    }
}
