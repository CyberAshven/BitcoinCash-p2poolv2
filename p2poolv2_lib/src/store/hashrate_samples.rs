// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! One sample of the hashrates the dashboard charts, and how the store
//! keeps it across restarts.
//!
//! The API's sampler records a sample once a minute into an in-memory ring
//! and queues the same sample to the store's `HashrateSamples` column
//! family, so a restart reloads the history instead of starting empty.
//! Retention is a hardcoded 30 days: the ring holds the newest
//! [`RETENTION_SECS`] samples at one a minute, the store evicts anything
//! older than [`RETENTION_SECS`] seconds, and the `/hashrate` window
//! accepts at most that many seconds.

use bitcoin::consensus::encode::{self, Decodable, Encodable};
use bitcoin::io::{Read, Write};

/// How long the store keeps hashrate samples, in seconds: 30 days.
/// Single source of truth: the API derives its ring bound and window
/// cap from this constant.
pub const RETENTION_SECS: u64 = 30 * 24 * 60 * 60;

/// The hashrates the dashboard charts at one minute, in hashes per second.
/// `None` where the value could not be known at that time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HashrateSample {
    /// Unix seconds the sample was taken.
    pub time: u64,
    /// The whole pool, from the share chain.
    pub pool: Option<f64>,
    /// This node's hashrate: the work of this node's accepted submissions
    /// in the trailing ten minutes over that window's span (p2poolv1's
    /// local hash rate), sampled at the API's cadence.
    pub local: Option<f64>,
    /// The Bitcoin Cash network, from the block template's difficulty.
    pub network: Option<f64>,
}

/// Tag byte marking an `Option<f64>` that carries a value.
const SOME_TAG: u8 = 1;
/// Tag byte marking an `Option<f64>` that is `None`.
const NONE_TAG: u8 = 0;

impl HashrateSample {
    /// The sample's column family key: its timestamp, big endian, so
    /// iteration is ordered oldest first and range deletes cut cleanly.
    /// Matches `SimplePplnsShare::make_key` and `user.rs` ordering.
    pub fn key(&self) -> [u8; 8] {
        Self::key_at(self.time)
    }

    /// The key a sample stamped `time_secs` sorts under.
    pub fn key_at(time_secs: u64) -> [u8; 8] {
        time_secs.to_be_bytes()
    }
}

fn encode_f64_or_none<W: Write + ?Sized>(
    value: &Option<f64>,
    writer: &mut W,
) -> Result<usize, bitcoin::io::Error> {
    match value {
        Some(value) => {
            let tag = SOME_TAG.consensus_encode(writer)?;
            let bytes = value.to_le_bytes();
            writer.write_all(&bytes)?;
            Ok(tag + bytes.len())
        }
        None => Ok(NONE_TAG.consensus_encode(writer)?),
    }
}

fn decode_f64_or_none<R: Read + ?Sized>(reader: &mut R) -> Result<Option<f64>, encode::Error> {
    match u8::consensus_decode(reader)? {
        SOME_TAG => {
            let mut bytes = [0u8; 8];
            reader.read_exact(&mut bytes)?;
            Ok(Some(f64::from_le_bytes(bytes)))
        }
        NONE_TAG => Ok(None),
        _tag => Err(encode::Error::ParseFailed(
            "unknown Option<f64> tag in hashrate sample",
        )),
    }
}

impl Encodable for HashrateSample {
    fn consensus_encode<W: Write + ?Sized>(
        &self,
        writer: &mut W,
    ) -> Result<usize, bitcoin::io::Error> {
        let mut length = self.time.consensus_encode(writer)?;
        length += encode_f64_or_none(&self.pool, writer)?;
        length += encode_f64_or_none(&self.local, writer)?;
        length += encode_f64_or_none(&self.network, writer)?;
        Ok(length)
    }
}

impl Decodable for HashrateSample {
    fn consensus_decode<R: Read + ?Sized>(reader: &mut R) -> Result<Self, encode::Error> {
        Ok(HashrateSample {
            time: u64::consensus_decode(reader)?,
            pool: decode_f64_or_none(reader)?,
            local: decode_f64_or_none(reader)?,
            network: decode_f64_or_none(reader)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::consensus::serialize;

    fn sample(time: u64) -> HashrateSample {
        HashrateSample {
            time,
            pool: Some(1.5),
            local: None,
            network: Some(2.5),
        }
    }

    #[test]
    fn a_sample_round_trips_through_consensus_encoding() {
        let original = sample(1_792_672_402);
        let bytes = serialize(&original);
        let decoded = encode::deserialize::<HashrateSample>(&bytes).expect("decodes");
        assert_eq!(decoded, original);
    }

    #[test]
    fn all_none_round_trips() {
        let original = HashrateSample {
            time: 0,
            pool: None,
            local: None,
            network: None,
        };
        let decoded =
            encode::deserialize::<HashrateSample>(&serialize(&original)).expect("decodes");
        assert_eq!(decoded, original);
    }

    #[test]
    fn keys_sort_by_timestamp_in_big_endian_order() {
        let older = HashrateSample::key_at(1_000);
        let newer = HashrateSample::key_at(1_001);
        assert!(older < newer);
        let last = HashrateSample::key_at(u64::MAX - 1);
        let max = HashrateSample::key_at(u64::MAX);
        assert!(last < max);
    }

    #[test]
    fn an_unknown_option_tag_is_an_error() {
        // time 0 (8 zero bytes) then tag 0xFF where pool's Option begins.
        let mut bytes = vec![0u8; 9];
        bytes[8] = 0xFF;
        let result = encode::deserialize::<HashrateSample>(&bytes);
        assert!(result.is_err());
    }

    #[test]
    fn retention_is_thirty_days() {
        assert_eq!(RETENTION_SECS, 2_592_000);
    }

    #[test]
    fn key_at_matches_the_samples_own_key() {
        let original = sample(1_234);
        assert_eq!(original.key(), HashrateSample::key_at(original.time));
    }

    #[test]
    fn encoding_carries_the_retention_constant() {
        let bytes = serialize(&sample(0));
        assert!(!bytes.is_empty());
        assert_eq!(RETENTION_SECS, 30 * 24 * 60 * 60);
    }
}
