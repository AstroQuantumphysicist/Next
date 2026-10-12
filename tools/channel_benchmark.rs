//! Host-only benchmark for the production helix-core framed queue.
//!
//! Large logical payloads are deliberately fragmented because one frame is
//! capped at 256 payload bytes. Results include frame encoding, queue API work,
//! and fixed-width handle attachment transport; they do not measure kernel
//! authorization, syscalls, devices, or guest latency.
use helix_core::{
    channel::{Channels, MAX_FRAME, MAX_HANDLES, MAX_PAYLOAD, QUEUE_DEPTH},
    handle::Handle,
    ipc::{self, Header, HEADER_SIZE},
};
use std::time::Instant;

const SAMPLES: usize = 5;
const SMALL_ITERATIONS: usize = 20_000;
const FOUR_KIB_ITERATIONS: usize = 1_024;
const SIXTY_FOUR_KIB_ITERATIONS: usize = 128;

#[derive(Clone, Copy)]
struct Workload {
    name: &'static str,
    payload_bytes: usize,
    iterations: usize,
    handles_per_frame: usize,
}

fn transfer(workload: Workload, verify_payload: bool) -> u128 {
    let fragments_per_message = workload.payload_bytes.div_ceil(MAX_PAYLOAD);
    let frame_count = workload.iterations * fragments_per_message;
    let payload: Vec<u8> = (0..workload.payload_bytes)
        .map(|index| (index.wrapping_mul(37).wrapping_add(11) & 0xff) as u8)
        .collect();
    let encoded_handles: [Handle; MAX_HANDLES] =
        std::array::from_fn(|index| Handle::from_raw(0x1000 + index as u64));
    let attachment_values: [u64; MAX_HANDLES] = std::array::from_fn(|index| 0x1000 + index as u64);

    let mut channels = Channels::<1, u64>::new();
    let [sender, receiver] = channels.create().expect("create channel");
    let mut encoded = [0; MAX_FRAME];
    let mut received = [0; MAX_FRAME];
    let start = Instant::now();
    let mut first_frame = 0;

    while first_frame < frame_count {
        let batch_len = (frame_count - first_frame).min(QUEUE_DEPTH);
        for frame_index in first_frame..first_frame + batch_len {
            let fragment = frame_index % fragments_per_message;
            let payload_start = fragment * MAX_PAYLOAD;
            let payload_end = (payload_start + MAX_PAYLOAD).min(payload.len());
            let chunk = &payload[payload_start..payload_end];
            let bytes = ipc::encode(
                &mut encoded,
                Header {
                    opcode: 7,
                    flags: 0,
                    correlation_id: frame_index as u64 + 1,
                    deadline: 0,
                },
                chunk,
                &encoded_handles[..workload.handles_per_frame],
            )
            .expect("encode bounded channel frame");
            channels
                .send_attached(
                    sender,
                    bytes,
                    &attachment_values[..workload.handles_per_frame],
                    0,
                )
                .expect("send bounded channel frame");
        }

        for frame_index in first_frame..first_frame + batch_len {
            let fragment = frame_index % fragments_per_message;
            let payload_start = fragment * MAX_PAYLOAD;
            let payload_end = (payload_start + MAX_PAYLOAD).min(payload.len());
            let chunk = &payload[payload_start..payload_end];
            let expected_length = HEADER_SIZE + chunk.len() + workload.handles_per_frame * 8;

            if workload.handles_per_frame == 0 {
                let length = channels
                    .receive(receiver, &mut received, 0)
                    .expect("receive bounded channel frame");
                assert_eq!(length, expected_length, "received frame length");
                if verify_payload {
                    verify_frame(&received[..length], frame_index, chunk, 0);
                }
            } else {
                let (length, peeked) = channels
                    .peek(receiver, &mut received, 0)
                    .expect("peek bounded channel frame");
                assert_eq!(length, expected_length, "peeked frame length");
                if verify_payload {
                    for index in 0..MAX_HANDLES {
                        let expected = (index < workload.handles_per_frame)
                            .then_some(attachment_values[index]);
                        assert_eq!(peeked[index], expected, "peeked attachment value");
                    }
                    verify_frame(
                        &received[..length],
                        frame_index,
                        chunk,
                        workload.handles_per_frame,
                    );
                }
                let taken = channels
                    .take(receiver)
                    .expect("take transferred references");
                if verify_payload {
                    for index in 0..MAX_HANDLES {
                        let expected = (index < workload.handles_per_frame)
                            .then_some(attachment_values[index]);
                        assert_eq!(taken[index], expected, "taken attachment value");
                    }
                }
            }
        }
        first_frame += batch_len;
    }

    start.elapsed().as_nanos()
}

fn verify_frame(bytes: &[u8], frame_index: usize, payload: &[u8], handles: usize) {
    let frame = ipc::decode(bytes, MAX_PAYLOAD, MAX_HANDLES, 0).expect("decode received frame");
    assert_eq!(frame.header.correlation_id, frame_index as u64 + 1);
    assert_eq!(frame.payload, payload);
    assert_eq!(frame.handle_count(), handles);
}

fn main() {
    let workloads = [
        Workload {
            name: "payload_1B",
            payload_bytes: 1,
            iterations: SMALL_ITERATIONS,
            handles_per_frame: 0,
        },
        Workload {
            name: "payload_64B",
            payload_bytes: 64,
            iterations: SMALL_ITERATIONS,
            handles_per_frame: 0,
        },
        Workload {
            name: "payload_4KiB_fragmented",
            payload_bytes: 4 * 1024,
            iterations: FOUR_KIB_ITERATIONS,
            handles_per_frame: 0,
        },
        Workload {
            name: "payload_64KiB_fragmented",
            payload_bytes: 64 * 1024,
            iterations: SIXTY_FOUR_KIB_ITERATIONS,
            handles_per_frame: 0,
        },
        Workload {
            name: "payload_64B_plus_1_attachment",
            payload_bytes: 64,
            iterations: SMALL_ITERATIONS,
            handles_per_frame: 1,
        },
        Workload {
            name: "payload_64B_plus_4_attachments",
            payload_bytes: 64,
            iterations: SMALL_ITERATIONS,
            handles_per_frame: MAX_HANDLES,
        },
    ];

    for workload in workloads {
        // Validate the whole logical payload once outside timed samples. This
        // checks fragmentation/reassembly order and attachment identity without
        // adding extra parser work to the recorded queue timings.
        transfer(
            Workload {
                iterations: 1,
                ..workload
            },
            true,
        );
        for sample in 0..SAMPLES {
            let elapsed_ns = transfer(workload, false);
            let fragments = workload.payload_bytes.div_ceil(MAX_PAYLOAD);
            println!(
                "SAMPLE\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                workload.name,
                workload.payload_bytes,
                workload.iterations,
                workload.handles_per_frame,
                fragments,
                sample,
                elapsed_ns
            );
        }
    }
    println!("VALIDATION\tpassed");
}
