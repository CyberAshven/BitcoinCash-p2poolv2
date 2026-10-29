// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Public-data-only claim and genesis transaction construction.

use libauth::{
    ResolvedTransactionCommon, create_virtual_machine_bch_2026, encode_data_push,
    encode_transaction_bch,
};

use crate::{
    Covenant, Input, LOCKTIME_THRESHOLD, Leaf, MAX_FEE_RATE, MAX_PAYOUT_SCRIPT_BYTES,
    MAX_SPLIT_BPS, MAX_UNLOCKING_BYTES, MIN_FEE_RATE, Nft, NonFungibleTokenCapability, OUTPUT_SATS,
    Output, Proof, REWARD_DIVISOR, Shares, SplitCommitment, State, Token, TransactionCommon,
    claim_target_bits, coinbase_height, decode_header_bits, decode_proof, decode_state,
    encode_state, hash256, leaf_hash, read_uint32, split_ext, split_preimage, split_shares,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Vault {
    /// Token category in display order, as a transaction identifier is shown.
    pub category: Vec<u8>,
    pub value: u64,
    pub token_amount: u64,
    pub commitment: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Split {
    pub donation_bps: u16,
    pub donation_locking_bytecode: Vec<u8>,
    pub fee_bps: u16,
    pub fee_locking_bytecode: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outpoint {
    pub txid: Vec<u8>,
    pub vout: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaimLeaf {
    pub state: State,
    pub height_delta: u32,
    pub target_bits: u32,
    pub preimage: Vec<u8>,
    pub payout_hash: Vec<u8>,
    pub leaf: Leaf,
    pub leaf_hash: Vec<u8>,
}

/// Build the height-dependent token leaf for a vault and payout commitment.
pub fn claim_leaf(
    covenant: &Covenant,
    vault: &Vault,
    payout: &[u8],
    split: Option<&Split>,
    height: u32,
) -> Result<ClaimLeaf, String> {
    let state = decode_state(&vault.commitment)?;
    if height >= LOCKTIME_THRESHOLD {
        return Err("the height must be a block height".into());
    }
    if state.height >= height {
        return Err("the coinbase is not newer than the thread state".into());
    }
    if payout.len() > MAX_PAYOUT_SCRIPT_BYTES
        || split.is_some_and(|split| {
            split.donation_locking_bytecode.len() > MAX_PAYOUT_SCRIPT_BYTES
                || split.fee_locking_bytecode.len() > MAX_PAYOUT_SCRIPT_BYTES
        })
    {
        return Err(
            "claim outputs pay scripts of at most 35 bytes: P2PKH, P2SH20 or P2SH32".into(),
        );
    }
    let height_delta = height - state.height;
    let target_bits = claim_target_bits(state.target_bits, height_delta, covenant.threads)?;
    let preimage = match split {
        None => Vec::with_capacity(0),
        Some(split) => split_preimage(&SplitCommitment {
            donation_bps: split.donation_bps,
            donation_hash: if split.donation_bps == 0 {
                vec![0; 32]
            } else {
                hash256(&split.donation_locking_bytecode)
            },
            fee_bps: split.fee_bps,
            fee_hash: if split.fee_bps == 0 {
                vec![0; 32]
            } else {
                hash256(&split.fee_locking_bytecode)
            },
        })?,
    };
    let payout_hash = hash256(payout);
    let leaf = Leaf {
        category: vault.category.iter().rev().copied().collect(),
        lock_hash: covenant.lock_hash.clone(),
        state_hash: hash256(&vault.commitment),
        payout_hash: payout_hash.clone(),
        target_bits,
        ext: split_ext(&preimage),
    };
    let leaf_hash = leaf_hash(&leaf)?;
    Ok(ClaimLeaf {
        state,
        height_delta,
        target_bits,
        preimage,
        payout_hash,
        leaf,
        leaf_hash,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaimPlan {
    pub proof: Vec<u8>,
    pub parts: Proof,
    pub height: u32,
    pub height_delta: u32,
    pub target_bits: u32,
    pub leaf: Leaf,
    pub leaf_hash: Vec<u8>,
    pub preimage: Vec<u8>,
    pub payout_hash: Vec<u8>,
    pub reward: u64,
    pub shares: Shares,
    pub outputs: Vec<Output>,
    pub next_state: Vec<u8>,
    pub size_bound: usize,
}

fn fungible_output(vault: &Vault, amount: u64, script: &[u8]) -> Output {
    Output {
        locking_bytecode: script.to_vec(),
        value_satoshis: OUTPUT_SATS,
        token: Some(Token {
            category: vault.category.clone(),
            amount,
            nft: None,
        }),
    }
}

/// Plan all outputs, state fields, and fee bounds for one claim.
pub fn plan_claim(
    covenant: &Covenant,
    vault: &Vault,
    payout: &[u8],
    split: Option<&Split>,
    proof: &[u8],
) -> Result<ClaimPlan, String> {
    let parts = decode_proof(proof)?;
    let height = coinbase_height(&parts.head)?;
    let info = claim_leaf(covenant, vault, payout, split, height)?;
    let reward = vault.token_amount / REWARD_DIVISOR;
    if reward == 0 {
        return Err("the vault has too few tokens for a reward".into());
    }
    let shares = match split {
        None => Shares {
            miner: reward,
            donation: 0,
            fee: 0,
        },
        Some(split) => {
            if split.donation_bps > MAX_SPLIT_BPS || split.fee_bps > MAX_SPLIT_BPS {
                return Err("a split slot is not canonical".into());
            }
            split_shares(
                reward,
                split.donation_bps,
                split.fee_bps,
                u128::from(vault.value) + u128::from(covenant.virtual_sats),
                vault.token_amount,
            )
        }
    };
    let mut outputs = Vec::with_capacity(3);
    if shares.miner > 0 {
        outputs.push(fungible_output(vault, shares.miner, payout));
    }
    if let Some(split) = split {
        if shares.donation > 0 {
            outputs.push(fungible_output(
                vault,
                shares.donation,
                &split.donation_locking_bytecode,
            ));
        }
        if shares.fee > 0 {
            outputs.push(fungible_output(
                vault,
                shares.fee,
                &split.fee_locking_bytecode,
            ));
        }
    }
    let next_state = encode_state(&State {
        thread: info.state.thread,
        height,
        target_bits: info.target_bits,
        ref_bits: read_uint32(&parts.header, 72),
    })?;
    let size_bound = covenant
        .fee_k
        .checked_add(proof.len())
        .ok_or("claim size overflow")?;
    Ok(ClaimPlan {
        proof: proof.to_vec(),
        parts,
        height,
        height_delta: info.height_delta,
        target_bits: info.target_bits,
        leaf: info.leaf,
        leaf_hash: info.leaf_hash,
        preimage: info.preimage,
        payout_hash: info.payout_hash,
        reward,
        shares,
        outputs,
        next_state,
        size_bound,
    })
}

/// A market spend unlocks with the redeem script alone.
pub fn market_unlocking_bytecode(covenant: &Covenant) -> Vec<u8> {
    encode_data_push(&covenant.redeem_script)
}

/// Build the three claim arguments and redeem push, enforcing the relay limit.
pub fn claim_unlocking_bytecode(
    covenant: &Covenant,
    preimage: &[u8],
    payout_hash: &[u8],
    proof: &[u8],
) -> Result<Vec<u8>, String> {
    let mut unlocking = Vec::with_capacity(
        preimage.len() + payout_hash.len() + proof.len() + covenant.redeem_script.len() + 12,
    );
    unlocking.extend_from_slice(&encode_data_push(preimage));
    unlocking.extend_from_slice(&encode_data_push(payout_hash));
    unlocking.extend_from_slice(&encode_data_push(proof));
    unlocking.extend_from_slice(&encode_data_push(&covenant.redeem_script));
    if unlocking.len() > MAX_UNLOCKING_BYTES {
        return Err("the claim exceeds the 10,000-byte unlocking limit".into());
    }
    Ok(unlocking)
}

/// The actual vault output used as the claim's source output in the VM.
pub fn vault_source_output(covenant: &Covenant, vault: &Vault) -> Output {
    Output {
        locking_bytecode: covenant.locking_bytecode.clone(),
        value_satoshis: vault.value,
        token: Some(Token {
            category: vault.category.clone(),
            amount: vault.token_amount,
            nft: Some(Nft {
                capability: NonFungibleTokenCapability::Mutable,
                commitment: vault.commitment.clone(),
            }),
        }),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuiltClaim {
    pub plan: ClaimPlan,
    pub fee: u64,
    pub size: usize,
    pub transaction: TransactionCommon,
    pub source_outputs: Vec<Output>,
}

/// Construct a complete unsigned claim; the covenant requires no signature.
#[allow(clippy::too_many_arguments)]
pub fn build_claim(
    covenant: &Covenant,
    outpoint: &Outpoint,
    vault: &Vault,
    payout: &[u8],
    split: Option<&Split>,
    proof: &[u8],
    fee_rate: u32,
    locktime: Option<u32>,
) -> Result<BuiltClaim, String> {
    if !(MIN_FEE_RATE..=MAX_FEE_RATE).contains(&fee_rate) {
        return Err("fee_rate must be 1, 2 or 3 satoshis per byte".into());
    }
    if outpoint.txid.len() != 32 {
        return Err("outpoint.txid must be 32 bytes".into());
    }
    let plan = plan_claim(covenant, vault, payout, split, proof)?;
    let fee = u64::try_from(plan.size_bound)
        .map_err(|_| "claim fee overflow")?
        .checked_mul(u64::from(fee_rate))
        .ok_or("claim fee overflow")?;
    let vault_value = vault
        .value
        .checked_sub(fee)
        .and_then(|value| value.checked_sub(OUTPUT_SATS * plan.outputs.len() as u64))
        .filter(|value| *value >= OUTPUT_SATS)
        .ok_or("the vault cannot pay the claim fee and outputs and keep 800 satoshis")?;
    let locktime = locktime.unwrap_or(plan.height);
    if locktime < plan.height || locktime >= LOCKTIME_THRESHOLD {
        return Err("the locktime must be a height at or above the coinbase height".into());
    }
    let source_outputs = vec![vault_source_output(covenant, vault)];
    let mut outputs = Vec::with_capacity(1 + plan.outputs.len());
    outputs.push(Output {
        locking_bytecode: covenant.locking_bytecode.clone(),
        value_satoshis: vault_value,
        token: Some(Token {
            category: vault.category.clone(),
            amount: vault.token_amount - plan.reward,
            nft: Some(Nft {
                capability: NonFungibleTokenCapability::Mutable,
                commitment: plan.next_state.clone(),
            }),
        }),
    });
    outputs.extend_from_slice(&plan.outputs);
    let transaction = TransactionCommon {
        version: 2,
        locktime,
        inputs: vec![Input {
            outpoint_transaction_hash: outpoint.txid.clone(),
            outpoint_index: outpoint.vout,
            sequence_number: 0xfffffffe,
            unlocking_bytecode: claim_unlocking_bytecode(
                covenant,
                &plan.preimage,
                &plan.payout_hash,
                proof,
            )?,
        }],
        outputs,
    };
    let size = encode_transaction_bch(&transaction).len();
    Ok(BuiltClaim {
        plan,
        fee,
        size,
        transaction,
        source_outputs,
    })
}

/// Verify a resolved transaction with BCH 2026 standard relay rules enabled.
pub fn verify_transaction(
    transaction: &TransactionCommon,
    source_outputs: &[Output],
) -> Result<(), String> {
    create_virtual_machine_bch_2026(Some(true)).verify(&ResolvedTransactionCommon {
        transaction: transaction.clone(),
        source_outputs: source_outputs.to_vec(),
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Funding {
    pub txid: Vec<u8>,
    pub vout: u32,
    pub value_satoshis: u64,
    pub locking_bytecode: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenesisOptions {
    pub funding: Funding,
    pub change_locking_bytecode: Vec<u8>,
    pub supply_per_thread: u64,
    pub seed_sats: u64,
    pub start_height: u32,
    pub target_bits: u32,
    pub ref_bits: u32,
    pub fee_rate: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuiltGenesis {
    pub transaction: TransactionCommon,
    pub fee: u64,
    pub size: usize,
    pub vaults: Vec<Output>,
}

/// Prepare genesis vaults and change without signing or creating a minting baton.
pub fn build_genesis(
    covenant: &Covenant,
    options: &GenesisOptions,
) -> Result<BuiltGenesis, String> {
    if options.funding.vout != 0 {
        return Err("the funding output must be output 0 of its transaction".into());
    }
    if options.funding.txid.len() != 32 {
        return Err("funding.txid must be 32 bytes".into());
    }
    if decode_header_bits(options.ref_bits).is_none() {
        return Err("ref_bits must be a valid nBits at most powLimit".into());
    }
    if options.supply_per_thread < REWARD_DIVISOR
        || u128::from(options.supply_per_thread) * u128::from(covenant.threads) > i64::MAX as u128
    {
        return Err("supply_per_thread must give a reward and fit the 2^63 - 1 token limit".into());
    }
    if options.seed_sats < 10_000 {
        return Err("seed_sats must be at least 10,000 satoshis per thread".into());
    }
    if options.fee_rate == 0 {
        return Err("fee_rate must be positive".into());
    }
    let mut vaults = Vec::with_capacity(covenant.threads as usize);
    for thread in 0..covenant.threads {
        vaults.push(Output {
            locking_bytecode: covenant.locking_bytecode.clone(),
            value_satoshis: options.seed_sats,
            token: Some(Token {
                category: options.funding.txid.clone(),
                amount: options.supply_per_thread,
                nft: Some(Nft {
                    capability: NonFungibleTokenCapability::Mutable,
                    commitment: encode_state(&State {
                        thread,
                        height: options.start_height,
                        target_bits: options.target_bits,
                        ref_bits: options.ref_bits,
                    })?,
                }),
            }),
        });
    }
    let mut outputs = Vec::with_capacity(vaults.len() + 1);
    outputs.extend_from_slice(&vaults);
    outputs.push(Output {
        locking_bytecode: options.change_locking_bytecode.clone(),
        value_satoshis: 0,
        token: None,
    });
    let mut transaction = TransactionCommon {
        version: 2,
        locktime: 0,
        inputs: vec![Input {
            outpoint_transaction_hash: options.funding.txid.clone(),
            outpoint_index: 0,
            sequence_number: 0xffffffff,
            unlocking_bytecode: Vec::with_capacity(0),
        }],
        outputs,
    };
    // Worst-case P2PKH ECDSA signature and public-key pushes add 108 bytes.
    let size = encode_transaction_bch(&transaction)
        .len()
        .checked_add(108)
        .ok_or("genesis size overflow")?;
    let fee = (size as u64)
        .checked_mul(u64::from(options.fee_rate))
        .ok_or("genesis fee overflow")?;
    let seeded = options
        .seed_sats
        .checked_mul(u64::from(covenant.threads))
        .ok_or("genesis seed overflow")?;
    let change = options
        .funding
        .value_satoshis
        .checked_sub(seeded)
        .and_then(|value| value.checked_sub(fee))
        .filter(|value| *value >= 546)
        .ok_or("the funding output cannot pay the vaults, fee and change")?;
    transaction
        .outputs
        .last_mut()
        .ok_or("genesis has no change output")?
        .value_satoshis = change;
    Ok(BuiltGenesis {
        transaction,
        fee,
        size,
        vaults,
    })
}
