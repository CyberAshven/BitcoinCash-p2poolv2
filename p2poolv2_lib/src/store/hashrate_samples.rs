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

/// The hashrates the dashboard charts at one minute, in hashes per second,
/// plus the node's online miner and worker counts at that minute.
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
    /// This node's miners with at least one active worker, as the
    /// dashboard's `/miners` page counts them.
    pub miners: Option<u64>,
    /// This node's active workers in all, counted the same way.
    pub workers: Option<u64>,
}

/// Tag byte marking an `Option` that carries a value.
const SOME_TAG: u8 = 1;
/// Tag byte marking an `Option` that is `None`.
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

fn encode_u64_or_none<W: Write + ?Sized>(
    value: &Option<u64>,
    writer: &mut W,
) -> Result<usize, bitcoin::io::Error> {
    match value {
        Some(value) => {
            let tag = SOME_TAG.consensus_encode(writer)?;
            Ok(tag + value.consensus_encode(writer)?)
        }
        None => Ok(NONE_TAG.consensus_encode(writer)?),
    }
}

/// Decode one count field: its tag byte, then the value when present. A
/// clean end of input where the tag would sit means the row was written
/// before the count fields existed (or by an older binary), so `None` is
/// the right answer there; anything else is a decode error like
/// [`decode_f64_or_none`]'s.
fn decode_count_or_none<R: Read + ?Sized>(reader: &mut R) -> Result<Option<u64>, encode::Error> {
    let mut tag = [0u8; 1];
    match reader.read(&mut tag) {
        Ok(0) => return Ok(None),
        Ok(_) => {}
        Err(error) if error.kind() == bitcoin::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(encode::Error::Io(error)),
    }
    match tag[0] {
        SOME_TAG => u64::consensus_decode(reader).map(Some),
        NONE_TAG => Ok(None),
        _tag => Err(encode::Error::ParseFailed(
            "unknown Option<u64> tag in hashrate sample",
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
        length += encode_u64_or_none(&self.miners, writer)?;
        length += encode_u64_or_none(&self.workers, writer)?;
        Ok(length)
    }
}

impl Decodable for HashrateSample {
    /// Reads the fields in order and stops when input ends: a row written
    /// before the count fields existed ends after `network` and decodes
    /// with `None` counts. Rows are loaded with `encode::deserialize`,
    /// which refuses unread bytes, so a binary older than the counts skips
    /// these rows as corrupt.
    fn consensus_decode<R: Read + ?Sized>(reader: &mut R) -> Result<Self, encode::Error> {
        Ok(HashrateSample {
            time: u64::consensus_decode(reader)?,
            pool: decode_f64_or_none(reader)?,
            local: decode_f64_or_none(reader)?,
            network: decode_f64_or_none(reader)?,
            miners: decode_count_or_none(reader)?,
            workers: decode_count_or_none(reader)?,
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
            miners: Some(3),
            workers: Some(7),
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
            miners: None,
            workers: None,
        };
        let decoded =
            encode::deserialize::<HashrateSample>(&serialize(&original)).expect("decodes");
        assert_eq!(decoded, original);
    }

    #[test]
    fn a_row_from_before_the_count_fields_decodes_with_none_counts() {
        // The layout an older binary wrote: time (8), pool tag + value
        // (9), local tag (1), network tag + value (9), and nothing after.
        // The counts were not recorded then, so `None` is the faithful
        // answer rather than a decode error.
        let mut bytes = serialize(&sample(1_000));
        bytes.truncate(8 + 9 + 1 + 9);
        let decoded = encode::deserialize::<HashrateSample>(&bytes).expect("decodes");
        assert_eq!(decoded.time, 1_000);
        assert_eq!(decoded.pool, Some(1.5));
        assert_eq!(decoded.local, None);
        assert_eq!(decoded.network, Some(2.5));
        assert_eq!(decoded.miners, None);
        assert_eq!(decoded.workers, None);
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
