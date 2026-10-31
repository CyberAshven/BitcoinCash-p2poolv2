// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0
// Ported from p2poolv2/p2poolv2 da18b991 and 543d6240 by Juwon Ogunseye.

#![no_main]

use bitcoin::consensus::{Decodable, Encodable};
use bitcoin::hashes::{Hash, sha256d};
use libfuzzer_sys::fuzz_target;
use p2poolv2_lib::node::messages::RawMessage;

// This smoke test checks that decoding never panics on arbitrary input. It
// does not establish that a decoded message has the expected values; that
// requires round-trip tests starting with generated Message values.
fuzz_target!(|payload: &[u8]| {
    // Exercise malformed framing as well as the correctly framed path below.
    let mut unframed_bytes = payload;
    let _ = RawMessage::consensus_decode(&mut unframed_bytes);

    // Compute the genuine checksum so Message::consensus_decode reaches the
    // fork's payload decoder, including its ShareHeaderBatch representation.
    let hash = sha256d::Hash::hash(payload);
    let checksum = [hash[0], hash[1], hash[2], hash[3]];
    let Ok(payload_length) = u32::try_from(payload.len()) else {
        return;
    };

    let mut wire_bytes = Vec::with_capacity(8 + payload.len());
    payload_length
        .consensus_encode(&mut wire_bytes)
        .expect("encoding into a vector cannot fail");
    checksum
        .consensus_encode(&mut wire_bytes)
        .expect("encoding into a vector cannot fail");
    wire_bytes.extend_from_slice(payload);

    // Both a successful decode and a clean decode error are acceptable.
    let _ = RawMessage::consensus_decode(&mut &wire_bytes[..]);
});
