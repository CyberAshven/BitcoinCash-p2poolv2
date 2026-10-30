// SPDX-License-Identifier: AGPL-3.0-or-later

//! Checked independent integer reference and canonical redeem state.

use std::collections::BTreeSet;

use libauth::hash256;

use crate::lean::program::compile_program;
use crate::reference::valid_payout_script;

pub const HEADER_BYTES: usize = 82;
pub const MAX_UNLOCK_BYTES: usize = 10_000;

/// Complete non-tail claim choices, canonically serialized by the caller.
///
/// `proof_seed` must include key, nonce, exact datasig, age, sequence, target,
/// amount layout, base reward, tail profile/range, locktime, BCH values and fee
/// rule from architecture section 4. This crate hashes these opaque bytes; it
/// does not validate a PHOTON claim or invent a share serialization.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProofBinding {
    pub parent: [u8; 32],
    pub baton_outpoint: [u8; 36],
    pub metadata_hash: [u8; 32],
    pub window_commitment: [u8; 32],
    pub ledger_commitment: [u8; 32],
    pub deployment_lock_hash: [u8; 32],
    pub proof_seed: Vec<u8>,
}

impl ProofBinding {
    /// Domain-separated binding with an explicit seed length; never a share ID.
    pub fn digest(&self) -> Result<[u8; 32], String> {
        let length = u32::try_from(self.proof_seed.len()).map_err(|_| "seed is too long")?;
        if self.proof_seed.is_empty() {
            return Err("proof seed must contain all non-tail choices".into());
        }
        let mut encoded = Vec::with_capacity(222 + self.proof_seed.len());
        encoded.extend_from_slice(b"PHOTON-LEAN-CONTEXT-v1");
        encoded.extend_from_slice(&self.parent);
        encoded.extend_from_slice(&self.baton_outpoint);
        encoded.extend_from_slice(&self.metadata_hash);
        encoded.extend_from_slice(&self.window_commitment);
        encoded.extend_from_slice(&self.ledger_commitment);
        encoded.extend_from_slice(&self.deployment_lock_hash);
        encoded.extend_from_slice(&length.to_le_bytes());
        encoded.extend_from_slice(&self.proof_seed);
        hash256(&encoded, None)
            .try_into()
            .map_err(|_| "HASH256 length".into())
    }
}

/// One directly committed beneficiary's gross debt and minimum multiplier.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub script: Vec<u8>,
    pub owed: u64,
    pub multiple: u16,
}

/// Who fixes the order in which the covenant pays committed entries.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ListOrder {
    /// Version 1: the covenant checks descending debt, then script bytes.
    #[default]
    MostOwed,
    /// Version 2: the share chain fixes the order, for example first
    /// eligible, first paid. The covenant pays in R's order and checks none.
    ShareChain,
}

impl ListOrder {
    /// The R header version that selects this program.
    pub fn version(self) -> u32 {
        match self {
            Self::MostOwed => 1,
            Self::ShareChain => 2,
        }
    }
}

/// Already ordered, bounded list. Version 1 orders by descending debt, then
/// script bytes; version 2 keeps the order the share chain committed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    pub order: ListOrder,
    pub network: u32,
    /// Raw wire order, opposite libauth's category display order.
    pub category: [u8; 32],
    pub context: [u8; 32],
    pub cost: u64,
    pub entries: Vec<Entry>,
}

impl Manifest {
    /// Validate exact priority order, unique scripts and supported minima.
    pub fn validate(&self) -> Result<(), String> {
        if self.network > 4 || self.cost == 0 || self.cost > i64::MAX as u64 / 10 {
            return Err("unsupported network or payout cost".into());
        }
        if self.entries.is_empty() || self.entries.len() > u16::MAX as usize {
            return Err("direct list must contain 1..65535 entries".into());
        }
        let mut scripts = BTreeSet::new();
        let mut previous: Option<&Entry> = None;
        for entry in &self.entries {
            let minimum = self
                .cost
                .checked_mul(u64::from(entry.multiple))
                .ok_or("minimum overflow")?;
            if !valid_payout_script(&entry.script)
                || entry.multiple < 10
                || entry.owed < minimum
                || entry.owed > i64::MAX as u64
            {
                return Err("invalid beneficiary, debt or minimum".into());
            }
            if !scripts.insert(entry.script.as_slice()) {
                return Err("duplicate beneficiary script".into());
            }
            if self.order == ListOrder::MostOwed
                && previous.is_some_and(|prior| {
                    prior.owed < entry.owed
                        || (prior.owed == entry.owed && prior.script >= entry.script)
                })
            {
                return Err("beneficiaries are not in canonical most-owed order".into());
            }
            previous = Some(entry);
        }
        Ok(())
    }

    /// Serialize the reference header and packed records for inspection.
    pub fn state(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        let mut encoded = Vec::with_capacity(HEADER_BYTES + self.entries.len() * 46);
        encoded.extend_from_slice(&self.order.version().to_le_bytes());
        encoded.extend_from_slice(&self.network.to_le_bytes());
        encoded.extend_from_slice(&self.category);
        encoded.extend_from_slice(&self.context);
        encoded.extend_from_slice(&self.cost.to_le_bytes());
        encoded.extend_from_slice(&(self.entries.len() as u16).to_le_bytes());
        for entry in &self.entries {
            encoded.push(entry.script.len() as u8);
            encoded.extend_from_slice(&entry.script);
            encoded.extend_from_slice(&entry.owed.to_le_bytes());
            encoded.extend_from_slice(&entry.multiple.to_le_bytes());
        }
        Ok(encoded)
    }

    /// Exact R: header, sentinel, reversed record pushes and fixed lean program.
    pub fn redeem(&self) -> Result<Vec<u8>, String> {
        let state = self.state()?;
        let mut redeem = libauth::encode_data_push(&state[..HEADER_BYTES]);
        // A sentinel below the individual immutable records proves exact count.
        // Reverse push order makes the highest-priority record pop first.
        redeem.extend_from_slice(&[0, 0x6b]);
        for entry in self.entries.iter().rev() {
            let mut record = Vec::with_capacity(46);
            record.push(entry.script.len() as u8);
            record.extend_from_slice(&entry.script);
            record.extend_from_slice(&entry.owed.to_le_bytes());
            record.extend_from_slice(&entry.multiple.to_le_bytes());
            redeem.extend_from_slice(&libauth::encode_data_push(&record));
            redeem.push(0x6b);
        }
        redeem.extend_from_slice(&compile_program(self.order)?);
        Ok(redeem)
    }

    /// Gross payouts, excluding committed candidates after the reward runs out.
    ///
    /// A clipped last payment below its minimum, or insufficient coverage,
    /// cannot faithfully satisfy the design. Return an error and leave tokens
    /// waiting; never pay ahead or manufacture a bootstrap exception.
    pub fn allocate(&self, reward: u64) -> Result<Vec<u64>, String> {
        self.validate()?;
        if reward == 0 || reward > i64::MAX as u64 {
            return Err("reward must be positive and fit signed 64 bits".into());
        }
        let mut remaining = reward;
        let mut result = Vec::with_capacity(self.entries.len());
        for entry in &self.entries {
            if remaining > 0 {
                let gross = entry.owed.min(remaining);
                let minimum = self
                    .cost
                    .checked_mul(u64::from(entry.multiple))
                    .ok_or("minimum overflow")?;
                if gross < minimum {
                    return Err("clipped final payout is below the miner minimum".into());
                }
                result.push(gross);
                remaining -= gross;
            }
        }
        if remaining != 0 {
            return Err("eligible capped list does not cover the actual reward".into());
        }
        Ok(result)
    }
}
