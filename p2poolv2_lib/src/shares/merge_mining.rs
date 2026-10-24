// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The merge-mining commitment in a Bitcoin Cash coinbase: output 0 in the
//! common format v2 that BCH P2Pool and Pickaxe both build and check.
//!
//! ```text
//! off len value
//! 0   8   00 00 00 00 00 00 00 00   value 0
//! 8   1   2c                        script length 44
//! 9   1   6a                        OP_RETURN
//! 10  1   2a                        push 42
//! 11  4   43 54 4d 4d               "CTMM"
//! 15  1   02                        version
//! 16  32  root                      as hashed, not reversed
//! 48  1   h                         tree height, 0..=16
//! 49  4   salt                      u32 LE
//! ```
//!
//! The exact empty commitment (a zero root with `h = 0` and salt 0) says the
//! coinbase merge-mines no token. A zero root with any other `h` or salt is
//! not a commitment. Every Bitcoin Cash coinbase P2Pool builds carries one of
//! these as output 0; Bitcoin coinbases carry none.
//!
//! The tree under the root, the token leaves and the proofs are in
//! [`super::auxpow`]. See `docs/bch-auxpow.md`.

use bitcoin::consensus::{Decodable, Encodable};
use bitcoin::{Amount, ScriptBuf, TxOut};
use serde::{Deserialize, Serialize};

/// Name of the merge-mining rules in Bitcoin Cash protocol strings: output 0
/// in common format v2 with its format checks, a pool-built share's token
/// root in its header (`roots`), and the ASERT header check of token shares.
/// A node on the old `fabe6d6d` output rebuilds another coinbase for every
/// share, and one without pool-built roots cannot read their headers.
pub const BCH_MERGE_MINING_RULE_TAG: &str = "mm2-roots-asert";

/// The tag that opens the commitment's data: `"CTMM"`.
pub const MERGE_MINING_MAGIC: [u8; 4] = *b"CTMM";

/// The format version this commitment carries.
pub const MERGE_MINING_VERSION: u8 = 0x02;

/// The tallest aux tree: `2^16` slots.
pub const MAX_MERGE_MINING_TREE_HEIGHT: u8 = 16;

/// Bytes pushed by the commitment's `OP_RETURN`: the magic, the version, the
/// root, `h` and the salt.
pub const MERGE_MINING_PAYLOAD_LENGTH: usize = 4 + 1 + 32 + 1 + 4;

/// The commitment's locking script: `OP_RETURN`, a one-byte push of 42 bytes,
/// then the payload.
pub const MERGE_MINING_SCRIPT_LENGTH: usize = 2 + MERGE_MINING_PAYLOAD_LENGTH;

/// The serialized output: an 8-byte zero value, a one-byte script length and
/// the script. 53 bytes.
pub const MERGE_MINING_OUTPUT_LENGTH: usize = 8 + 1 + MERGE_MINING_SCRIPT_LENGTH;

/// Root, tree height and salt of a coinbase's merge-mining commitment.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MergeMiningCommitment {
    /// Root of the aux tree, as hashed (never byte-reversed).
    pub root: [u8; 32],
    /// The tree's height `h`: `2^h` leaf slots, at most 16.
    pub height: u8,
    /// The salt the slots were placed with.
    pub salt: u32,
}

impl MergeMiningCommitment {
    /// The exact empty commitment: no token is merge-mined.
    pub const EMPTY: Self = Self {
        root: [0; 32],
        height: 0,
        salt: 0,
    };

    /// The exact empty commitment for a job that merge-mines no tokens.
    pub const fn empty() -> Self {
        Self::EMPTY
    }

    /// True for the exact empty commitment and nothing else.
    pub fn is_empty(&self) -> bool {
        *self == Self::EMPTY
    }

    /// The format checks every commitment passes: `h` at most 16, and a zero
    /// root only as the exact empty commitment.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.height > MAX_MERGE_MINING_TREE_HEIGHT {
            return Err("merge-mining tree height exceeds 16");
        }
        if self.root == [0; 32] && !self.is_empty() {
            return Err("a zero merge-mining root needs h = 0 and salt 0");
        }
        Ok(())
    }

    /// The 42 bytes pushed by the commitment output.
    pub fn payload(&self) -> [u8; MERGE_MINING_PAYLOAD_LENGTH] {
        let mut payload = [0; MERGE_MINING_PAYLOAD_LENGTH];
        payload[..4].copy_from_slice(&MERGE_MINING_MAGIC);
        payload[4] = MERGE_MINING_VERSION;
        payload[5..37].copy_from_slice(&self.root);
        payload[37] = self.height;
        payload[38..].copy_from_slice(&self.salt.to_le_bytes());
        payload
    }

    /// The 44-byte locking script: `OP_RETURN OP_PUSHBYTES_42 <payload>`.
    pub fn script(&self) -> ScriptBuf {
        let mut script = Vec::with_capacity(MERGE_MINING_SCRIPT_LENGTH);
        script.push(bitcoin::opcodes::all::OP_RETURN.to_u8());
        script.push(MERGE_MINING_PAYLOAD_LENGTH as u8);
        script.extend_from_slice(&self.payload());
        ScriptBuf::from_bytes(script)
    }

    /// The zero-value output a coinbase carries as output 0.
    pub fn output(&self) -> TxOut {
        TxOut {
            value: Amount::ZERO,
            script_pubkey: self.script(),
        }
    }

    /// Read a commitment output. Anything but a zero value and the exact
    /// 44-byte script with this magic and version, or a commitment failing
    /// [`Self::validate`], is not a commitment.
    pub fn from_output(output: &TxOut) -> Result<Self, &'static str> {
        let script = output.script_pubkey.as_bytes();
        if output.value != Amount::ZERO
            || script.len() != MERGE_MINING_SCRIPT_LENGTH
            || script[..2]
                != [
                    bitcoin::opcodes::all::OP_RETURN.to_u8(),
                    MERGE_MINING_PAYLOAD_LENGTH as u8,
                ]
            || script[2..6] != MERGE_MINING_MAGIC
            || script[6] != MERGE_MINING_VERSION
        {
            return Err("output is not a merge-mining v2 commitment");
        }
        let commitment = Self {
            root: script[7..39].try_into().expect("32-byte root"),
            height: script[39],
            salt: u32::from_le_bytes(script[40..44].try_into().expect("4-byte salt")),
        };
        commitment.validate()?;
        Ok(commitment)
    }

    /// Output list carrying [`Self::output`], for a Job Declaration token.
    ///
    /// The Stratum V2 Job Declaration specification defines
    /// `AllocateMiningJobToken.Success.coinbase_tx_outputs` as a
    /// CompactSize-prefixed array of consensus-serialized outputs, so the
    /// bytes are the one-element list, count first.
    pub fn coinbase_outputs_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(1 + MERGE_MINING_OUTPUT_LENGTH);
        vec![self.output()]
            .consensus_encode(&mut bytes)
            .expect("a vector writer does not fail");
        bytes
    }
}

/// Share header encoding: the root, `h` and the salt (37 bytes).
impl Encodable for MergeMiningCommitment {
    fn consensus_encode<W: bitcoin::io::Write + ?Sized>(
        &self,
        writer: &mut W,
    ) -> Result<usize, bitcoin::io::Error> {
        writer.write_all(&self.root)?;
        Ok(32 + self.height.consensus_encode(writer)? + self.salt.consensus_encode(writer)?)
    }
}

impl Decodable for MergeMiningCommitment {
    fn consensus_decode<R: bitcoin::io::Read + ?Sized>(
        reader: &mut R,
    ) -> Result<Self, bitcoin::consensus::encode::Error> {
        let mut root = [0; 32];
        reader.read_exact(&mut root)?;
        let commitment = Self {
            root,
            height: u8::consensus_decode(reader)?,
            salt: u32::consensus_decode(reader)?,
        };
        commitment
            .validate()
            .map_err(bitcoin::consensus::encode::Error::ParseFailed)?;
        Ok(commitment)
    }
}

#[cfg(test)]
mod tests;
