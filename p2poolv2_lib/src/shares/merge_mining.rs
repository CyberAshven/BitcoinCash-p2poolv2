// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! One AuxPoW commitment output in a Bitcoin Cash coinbase.
//!
//! The payload is the merged-mining commitment from the Bitcoin wiki
//! specification: magic `fabe6d6d`, a 32-byte aux merkle root, the tree size,
//! and the nonce. It sits in a single zero-value `OP_RETURN` output. A
//! reserved commitment (zero root, size 0, nonce 0) holds the output's place
//! when the miner has not declared an aux chain. Bitcoin coinbases omit it.

use bitcoin::consensus::{Decodable, Encodable};
use bitcoin::script::Builder;
use bitcoin::{Amount, ScriptBuf, TxOut};
use serde::{Deserialize, Serialize};

/// Magic that opens an AuxPoW commitment: `0xfa 0xbe 'm' 'm'`.
pub const AUXPOW_MAGIC: [u8; 4] = [0xfa, 0xbe, 0x6d, 0x6d];

/// Bytes in the commitment payload: magic, root, size, nonce.
pub const AUXPOW_COMMITMENT_LENGTH: usize = 4 + 32 + 4 + 4;

/// Aux merkle root, tree size, and nonce committed by one coinbase output.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeMiningCommitment {
    /// Merkle root of the aux chain hashes, in the byte order AuxPoW stores.
    pub merkle_root: [u8; 32],
    /// Number of leaf slots, a power of two; zero reserves an unused tree.
    pub tree_size: u32,
    /// Nonce used by AuxPoW chain-id slot selection.
    pub nonce: u32,
}

impl MergeMiningCommitment {
    /// Decode the one canonical zero-value aux commitment output.
    pub fn from_output(output: &TxOut) -> Result<Self, &'static str> {
        let script = output.script_pubkey.as_bytes();
        if output.value != Amount::ZERO
            || script.len() != 46
            || script[..2] != [0x6a, 44]
            || script[2..6] != AUXPOW_MAGIC
        {
            return Err("invalid aux commitment output");
        }
        let commitment = Self {
            merkle_root: script[6..38].try_into().expect("32-byte root"),
            tree_size: u32::from_le_bytes(script[38..42].try_into().expect("4-byte size")),
            nonce: u32::from_le_bytes(script[42..46].try_into().expect("4-byte nonce")),
        };
        commitment.validate()?;
        Ok(commitment)
    }

    /// Reject ambiguous empty trees and impossible AuxPoW tree widths.
    pub fn validate(self) -> Result<(), &'static str> {
        if self.tree_size == 0 {
            if self != Self::reserved() {
                return Err("unused aux tree must have a zero root and nonce");
            }
        } else if !self.tree_size.is_power_of_two() || self.tree_size > (1 << 30) {
            return Err("aux tree size must be a power of two no greater than 2^30");
        }
        Ok(())
    }

    /// Commitment that reserves the output and names no aux chain: a zero
    /// root, tree size and nonce.
    pub fn reserved() -> Self {
        Self::default()
    }

    /// The 44 bytes pushed by the commitment output.
    pub fn payload(self) -> [u8; AUXPOW_COMMITMENT_LENGTH] {
        let mut payload = [0u8; AUXPOW_COMMITMENT_LENGTH];
        payload[..4].copy_from_slice(&AUXPOW_MAGIC);
        payload[4..36].copy_from_slice(&self.merkle_root);
        payload[36..40].copy_from_slice(&self.tree_size.to_le_bytes());
        payload[40..44].copy_from_slice(&self.nonce.to_le_bytes());
        payload
    }

    /// Zero-value output carrying [`Self::payload`].
    pub fn output(self) -> TxOut {
        TxOut {
            value: Amount::ZERO,
            script_pubkey: commitment_script(self.payload()),
        }
    }

    /// Output list carrying [`Self::output`], for a Job Declaration token.
    ///
    /// The Stratum V2 Job Declaration specification defines
    /// `AllocateMiningJobToken.Success.coinbase_tx_outputs` as a
    /// CompactSize-prefixed array of consensus-serialized outputs, so the
    /// bytes are the one-element list, count first.
    pub fn coinbase_outputs_bytes(self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(64);
        vec![self.output()]
            .consensus_encode(&mut bytes)
            .expect("a vector writer does not fail");
        bytes
    }
}

impl Encodable for MergeMiningCommitment {
    fn consensus_encode<W: bitcoin::io::Write + ?Sized>(
        &self,
        writer: &mut W,
    ) -> Result<usize, bitcoin::io::Error> {
        writer.write_all(&self.merkle_root)?;
        Ok(32 + self.tree_size.consensus_encode(writer)? + self.nonce.consensus_encode(writer)?)
    }
}

impl Decodable for MergeMiningCommitment {
    fn consensus_decode<R: bitcoin::io::Read + ?Sized>(
        reader: &mut R,
    ) -> Result<Self, bitcoin::consensus::encode::Error> {
        let mut merkle_root = [0; 32];
        reader.read_exact(&mut merkle_root)?;
        let commitment = Self {
            merkle_root,
            tree_size: u32::consensus_decode(reader)?,
            nonce: u32::consensus_decode(reader)?,
        };
        commitment
            .validate()
            .map_err(bitcoin::consensus::encode::Error::ParseFailed)?;
        Ok(commitment)
    }
}

fn commitment_script(payload: [u8; AUXPOW_COMMITMENT_LENGTH]) -> ScriptBuf {
    Builder::new()
        .push_opcode(bitcoin::opcodes::all::OP_RETURN)
        .push_slice(payload)
        .into_script()
}

#[cfg(test)]
mod tests;
