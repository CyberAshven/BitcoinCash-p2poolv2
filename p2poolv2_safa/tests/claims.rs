// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Complete spends and attacks, enforced by the standard BCH 2026 VM.
mod test_utils;

use libauth::{Input, NonFungibleTokenCapability, Output, Token};
use p2poolv2_safa::{self as safa, Outpoint, Shares, State, Vault};
use test_utils::{headers::MAINNET_PAIRS, *};

#[test]
fn claim_without_split_pays_whole_reward() {
    let scenario = default_scenario();
    let claim = &scenario.claim;
    accepts(claim);
    assert_eq!(claim.transaction.outputs.len(), 2);
    assert_eq!(token(&claim.transaction.outputs[1]).amount, 1_000_000);
    assert_eq!(claim.transaction.outputs[1].value_satoshis, 800);
    assert_eq!(
        token(&claim.transaction.outputs[0]).amount,
        scenario.vault.token_amount - 1_000_000
    );
    assert_eq!(
        claim.transaction.outputs[0].value_satoshis,
        scenario.vault.value - claim.fee - 800
    );
    assert_eq!(claim.fee, claim.plan.size_bound as u64);
    assert!(claim.size <= claim.plan.size_bound);
    assert_eq!(
        safa::decode_state(
            &token(&claim.transaction.outputs[0])
                .nft
                .as_ref()
                .unwrap()
                .commitment
        )
        .unwrap(),
        State {
            thread: 1,
            height: block_height(),
            target_bits: claim.plan.target_bits,
            ref_bits: block_bits()
        }
    );
}
#[test]
fn golden_split_claim_pays_pickaxe_amounts_and_scripts() {
    let scenario = scenario(ScenarioOptions {
        covenant: one_sat(),
        split: Some(split()),
        ..Default::default()
    })
    .unwrap();
    let claim = &scenario.claim;
    accepts(claim);
    assert_eq!(claim.transaction.outputs.len(), 4);
    assert_eq!(token(&claim.transaction.outputs[1]).amount, 980100);
    assert_eq!(claim.transaction.outputs[1].value_satoshis, 800);
    assert_eq!(claim.transaction.outputs[1].locking_bytecode, p2pkh(0x12));
    assert_eq!(token(&claim.transaction.outputs[2]).amount, 10000);
    assert_eq!(claim.transaction.outputs[2].value_satoshis, 800);
    assert_eq!(claim.transaction.outputs[2].locking_bytecode, p2pkh(0xd0));
    assert_eq!(token(&claim.transaction.outputs[3]).amount, 9900);
    assert_eq!(claim.transaction.outputs[3].value_satoshis, 800);
    assert_eq!(claim.transaction.outputs[3].locking_bytecode, p2pkh(0xfe));
    assert!(claim.size <= claim.plan.size_bound);
}
#[test]
fn whole_donation_still_binds_payout_hash() {
    let mut donation = split();
    donation.donation_bps = 10000;
    donation.fee_bps = 0;
    donation.fee_locking_bytecode.clear();
    let scenario = scenario(ScenarioOptions {
        covenant: one_sat(),
        split: Some(donation),
        ..Default::default()
    })
    .unwrap();
    accepts(&scenario.claim);
    assert_eq!(scenario.claim.transaction.outputs.len(), 2);
    assert_eq!(
        token(&scenario.claim.transaction.outputs[1]).amount,
        1_000_000
    );
    let mut changed = scenario.claim.clone();
    set_unlock(
        &mut changed,
        &scenario.covenant,
        &scenario.claim.plan.preimage,
        &safa::hash256(&p2pkh(0x99)),
        &scenario.proof,
    );
    refuses(&changed);
}
#[test]
fn dust_donation_stays_with_miner_without_growing_fee() {
    let mut small = split();
    small.donation_bps = 99;
    small.fee_bps = 200;
    let scenario = scenario(ScenarioOptions {
        split: Some(small),
        ..Default::default()
    })
    .unwrap();
    accepts(&scenario.claim);
    assert_eq!(
        scenario.claim.plan.shares,
        Shares {
            miner: 980198,
            donation: 0,
            fee: 19802
        }
    );
    assert_eq!(scenario.claim.transaction.outputs.len(), 3);
    assert_eq!(
        scenario.claim.transaction.outputs[2].locking_bytecode,
        p2pkh(0xfe)
    );
    let mut paid = scenario.claim.clone();
    paid.transaction.outputs.insert(
        2,
        Output {
            locking_bytecode: p2pkh(0xd0),
            value_satoshis: 800,
            token: Some(Token {
                category: category(),
                amount: 9900,
                nft: None,
            }),
        },
    );
    token_mut(&mut paid.transaction.outputs[1]).amount -= 9900;
    paid.transaction.outputs[0].value_satoshis -= 800;
    refuses(&paid);
    let mut grown = scenario.claim.clone();
    token_mut(&mut grown.transaction.outputs[1]).amount -= 9900;
    token_mut(&mut grown.transaction.outputs[2]).amount += 9900;
    refuses(&grown);
}

macro_rules! claim_attack {
    ($name:ident,$edit:expr) => {
        #[test]
        fn $name() {
            let mut claim = default_scenario().claim;
            $edit(&mut claim);
            refuses(&claim);
        }
    };
}
claim_attack!(
    miner_cannot_receive_extra_token,
    |claim: &mut safa::BuiltClaim| {
        token_mut(&mut claim.transaction.outputs[0]).amount -= 1;
        token_mut(&mut claim.transaction.outputs[1]).amount += 1;
    }
);
claim_attack!(
    miner_cannot_receive_extra_satoshi,
    |claim: &mut safa::BuiltClaim| {
        claim.transaction.outputs[0].value_satoshis -= 1;
        claim.transaction.outputs[1].value_satoshis += 1;
    }
);
claim_attack!(claim_cannot_add_output, |claim: &mut safa::BuiltClaim| {
    claim.transaction.outputs.push(Output {
        locking_bytecode: vec![0x6a, b'x'],
        value_satoshis: 0,
        token: None,
    })
});
claim_attack!(
    claim_cannot_add_sponsor_input,
    |claim: &mut safa::BuiltClaim| {
        claim.source_outputs.push(Output {
            locking_bytecode: vec![0x51],
            value_satoshis: 100000,
            token: None,
        });
        claim.transaction.inputs.push(Input {
            outpoint_transaction_hash: safa::hash256(b"sponsor"),
            outpoint_index: 0,
            sequence_number: 0xffff_fffe,
            unlocking_bytecode: vec![],
        });
    }
);
claim_attack!(
    vault_cannot_move_to_another_lock,
    |claim: &mut safa::BuiltClaim| claim.transaction.outputs[0].locking_bytecode = p2pkh(0x99)
);
claim_attack!(
    vault_nft_cannot_become_immutable,
    |claim: &mut safa::BuiltClaim| token_mut(&mut claim.transaction.outputs[0])
        .nft
        .as_mut()
        .unwrap()
        .capability = NonFungibleTokenCapability::None
);
claim_attack!(
    vault_nft_cannot_move_to_miner,
    |claim: &mut safa::BuiltClaim| {
        let first = claim.transaction.outputs[0].token.take();
        let second = claim.transaction.outputs[1].token.take();
        claim.transaction.outputs[0].token = second;
        claim.transaction.outputs[1].token = first;
    }
);
claim_attack!(
    proof_cannot_claim_another_thread,
    |claim: &mut safa::BuiltClaim| {
        *commitment_mut(&mut claim.source_outputs[0]) = safa::encode_state(&State {
            thread: 2,
            height: block_height() - 10,
            target_bits: 0x2000_ffff,
            ref_bits: block_bits(),
        })
        .unwrap();
        commitment_mut(&mut claim.transaction.outputs[0])[0] = 2;
    }
);
claim_attack!(
    proof_cannot_claim_another_category,
    |claim: &mut safa::BuiltClaim| {
        let foreign = safa::hash256(b"another category");
        token_mut(&mut claim.source_outputs[0]).category = foreign.clone();
        token_mut(&mut claim.transaction.outputs[0]).category = foreign.clone();
        token_mut(&mut claim.transaction.outputs[1]).category = foreign;
    }
);
claim_attack!(
    next_state_thread_cannot_change,
    |claim: &mut safa::BuiltClaim| commitment_mut(&mut claim.transaction.outputs[0])[0] ^= 1
);
claim_attack!(
    next_state_height_cannot_change,
    |claim: &mut safa::BuiltClaim| commitment_mut(&mut claim.transaction.outputs[0])[4] ^= 1
);
claim_attack!(
    next_state_target_cannot_change,
    |claim: &mut safa::BuiltClaim| commitment_mut(&mut claim.transaction.outputs[0])[8] ^= 1
);
claim_attack!(
    next_state_reference_cannot_change,
    |claim: &mut safa::BuiltClaim| commitment_mut(&mut claim.transaction.outputs[0])[12] ^= 1
);
claim_attack!(
    cltv_refuses_early_locktime,
    |claim: &mut safa::BuiltClaim| claim.transaction.locktime = block_height() - 1
);
claim_attack!(
    cltv_refuses_time_locktime,
    |claim: &mut safa::BuiltClaim| claim.transaction.locktime = 1700000000
);
claim_attack!(
    cltv_refuses_final_sequence,
    |claim: &mut safa::BuiltClaim| claim.transaction.inputs[0].sequence_number = u32::MAX
);

#[test]
fn theft_cannot_redirect_miner_even_with_matching_payout_hash() {
    let scenario = scenario(ScenarioOptions {
        split: Some(split()),
        ..Default::default()
    })
    .unwrap();
    let mut changed = scenario.claim.clone();
    changed.transaction.outputs[1].locking_bytecode = p2pkh(0x99);
    refuses(&changed);
    set_unlock(
        &mut changed,
        &scenario.covenant,
        &scenario.claim.plan.preimage,
        &safa::hash256(&p2pkh(0x99)),
        &scenario.proof,
    );
    refuses(&changed);
}
macro_rules! split_attack {
    ($name:ident,$edit:expr) => {
        #[test]
        fn $name() {
            let mut split = split();
            split.fee_bps = 200;
            let mut claim = scenario(ScenarioOptions {
                covenant: one_sat(),
                split: Some(split),
                ..Default::default()
            })
            .unwrap()
            .claim;
            accepts(&claim);
            $edit(&mut claim);
            refuses(&claim);
        }
    };
}
split_attack!(
    donation_cannot_be_redirected,
    |claim: &mut safa::BuiltClaim| claim.transaction.outputs[2].locking_bytecode = p2pkh(0x99)
);
split_attack!(
    donation_and_fee_cannot_swap,
    |claim: &mut safa::BuiltClaim| claim.transaction.outputs.swap(2, 3)
);
split_attack!(
    miner_cannot_keep_donation,
    |claim: &mut safa::BuiltClaim| {
        let donation = token(&claim.transaction.outputs[2]).amount;
        token_mut(&mut claim.transaction.outputs[1]).amount += donation;
        claim.transaction.outputs.remove(2);
        claim.transaction.outputs[0].value_satoshis += 800;
    }
);
split_attack!(
    fee_token_cannot_move_to_donation,
    |claim: &mut safa::BuiltClaim| {
        token_mut(&mut claim.transaction.outputs[2]).amount += 1;
        token_mut(&mut claim.transaction.outputs[3]).amount -= 1;
    }
);
split_attack!(
    donation_cannot_get_801_satoshis,
    |claim: &mut safa::BuiltClaim| {
        claim.transaction.outputs[2].value_satoshis += 1;
        claim.transaction.outputs[0].value_satoshis -= 1;
    }
);
split_attack!(
    fee_cannot_use_foreign_tokens,
    |claim: &mut safa::BuiltClaim| token_mut(&mut claim.transaction.outputs[3]).category =
        safa::hash256(b"foreign tokens")
);
#[test]
fn split_preimage_cannot_be_withheld() {
    let scenario = scenario(ScenarioOptions {
        covenant: one_sat(),
        split: Some(split()),
        ..Default::default()
    })
    .unwrap();
    let mut changed = scenario.claim.clone();
    set_unlock(
        &mut changed,
        &scenario.covenant,
        &[],
        &scenario.claim.plan.payout_hash,
        &scenario.proof,
    );
    refuses(&changed);
}
#[test]
fn split_preimage_cannot_be_rewritten() {
    let mut split = split();
    split.fee_bps = 200;
    let scenario = scenario(ScenarioOptions {
        covenant: one_sat(),
        split: Some(split),
        ..Default::default()
    })
    .unwrap();
    let mut changed = scenario.claim.clone();
    changed.transaction.outputs[2].locking_bytecode = p2pkh(0x99);
    let preimage = safa::split_preimage(&safa::SplitCommitment {
        donation_bps: 100,
        donation_hash: safa::hash256(&p2pkh(0x99)),
        fee_bps: 200,
        fee_hash: safa::hash256(&p2pkh(0xfe)),
    })
    .unwrap();
    set_unlock(
        &mut changed,
        &scenario.covenant,
        &preimage,
        &scenario.claim.plan.payout_hash,
        &scenario.proof,
    );
    refuses(&changed);
}

#[test]
fn bitcoin_nbits_refused_even_with_token_work() {
    refuses(
        &scenario(ScenarioOptions {
            bits: 0x1703_0ecd,
            ..Default::default()
        })
        .unwrap()
        .claim,
    );
}
#[test]
fn almost_pow_limit_is_not_allowance() {
    refuses(
        &scenario(ScenarioOptions {
            bits: 0x1d00_fffe,
            ..Default::default()
        })
        .unwrap()
        .claim,
    );
}
#[test]
fn chipnet_minimum_difficulty_is_allowed() {
    accepts(
        &scenario(ScenarioOptions {
            bits: 0x1d00_ffff,
            ..Default::default()
        })
        .unwrap()
        .claim,
    );
}
macro_rules! fallback_claim {
    ($name:ident,$delta:expr,$bits:expr,$reference:expr,$accepted:expr) => {
        #[test]
        fn $name() {
            let mut options = ScenarioOptions {
                bits: $bits,
                ..Default::default()
            };
            options.state.height = block_height() - $delta;
            options.state.ref_bits = $reference;
            let scenario = scenario(options).unwrap();
            if $accepted {
                accepts(&scenario.claim);
            } else {
                refuses(&scenario.claim);
            }
        }
    };
}
fallback_claim!(
    fallback_exact_sixteenth_after_4032,
    4032,
    0x1b04_04cb,
    0x1b40_4cb0,
    true
);
fallback_claim!(
    fallback_reference_after_4032,
    4032,
    0x1b40_4cb0,
    0x1b40_4cb0,
    true
);
fallback_claim!(
    fallback_sixteenth_at_4031_refused,
    4031,
    0x1b04_04cb,
    0x1b40_4cb0,
    false
);
fallback_claim!(
    fallback_reference_at_4031_refused,
    4031,
    0x1b40_4cb0,
    0x1b40_4cb0,
    false
);
fallback_claim!(
    fallback_below_sixteenth_refused,
    4032,
    0x1b04_04ca,
    0x1b40_4cb0,
    false
);
fallback_claim!(
    fallback_stays_bounded_after_100000,
    100000,
    0x1b04_04ca,
    0x1b40_4cb0,
    false
);
fallback_claim!(
    fallback_wrong_reference_refused,
    4032,
    0x1b04_04cb,
    0x1a31_06ee,
    false
);
#[test]
fn fallback_cases_fail_normal_band() {
    assert!(!safa::asert_band_ok("chipnet", 0x1b40_4cb0, block_time(), block_height()).unwrap());
    assert!(!safa::asert_band_ok("chipnet", 0x1b04_04cb, block_time(), block_height()).unwrap());
    assert!(!safa::asert_band_ok("chipnet", 0x1b04_04ca, block_time(), block_height()).unwrap());
}

macro_rules! mainnet_claim {
    ($name:ident,$bits:expr,$accepted:expr) => {
        #[test]
        fn $name() {
            let pair = MAINNET_PAIRS[8];
            let mut options = ScenarioOptions {
                covenant: safa::create_covenant("mainnet", 4, 42_000_000_000).unwrap(),
                height: pair.0 + 1,
                time: header_time(pair.2),
                bits: $bits,
                ..Default::default()
            };
            options.state.height = pair.0 - 9;
            options.state.ref_bits = header_bits(pair.2);
            let scenario = scenario(options).unwrap();
            if $accepted {
                accepts(&scenario.claim);
            } else {
                refuses(&scenario.claim);
            }
        }
    };
}
mainnet_claim!(mainnet_claim_real_nbits, 0x1802_3101, true);
mainnet_claim!(mainnet_claim_refuses_pow_limit, 0x1d00_ffff, false);
mainnet_claim!(mainnet_claim_refuses_bitcoin_nbits, 0x1703_0ecd, false);
#[test]
fn reused_height_refused() {
    refuses(&raw_claim(
        block_height(),
        block_height(),
        block_bits(),
        block_height(),
        p2pkh(0x12),
    ));
}
#[test]
fn older_height_refused() {
    refuses(&raw_claim(
        block_height() + 5,
        block_height(),
        block_bits(),
        block_height(),
        p2pkh(0x12),
    ));
}
#[test]
fn one_newer_height_passes() {
    accepts(&raw_claim(
        block_height() - 1,
        block_height(),
        block_bits(),
        block_height(),
        p2pkh(0x12),
    ));
}
#[test]
fn claimed_proof_cannot_replay_and_successor_claim_passes() {
    let first = default_scenario();
    accepts(&first.claim);
    let vault = first.claim.transaction.outputs[0].clone();
    let commitment = &token(&vault).nft.as_ref().unwrap().commitment;
    let next = scenario(ScenarioOptions {
        state: safa::decode_state(commitment).unwrap(),
        value: vault.value_satoshis,
        token_amount: token(&vault).amount,
        height: block_height() + 1,
        time: block_time() + 600,
        ..Default::default()
    })
    .unwrap();
    accepts(&next.claim);
    let mut replay = first.claim.clone();
    replay.source_outputs = vec![vault.clone()];
    token_mut(&mut replay.transaction.outputs[0]).amount = token(&vault).amount - 1_000_000;
    replay.transaction.outputs[0].value_satoshis = vault.value_satoshis - first.claim.fee - 800;
    refuses(&replay);
}
#[test]
fn cltv_allows_later_height() {
    let mut claim = default_scenario().claim;
    claim.transaction.locktime = block_height() + 100;
    accepts(&claim);
}
#[test]
fn time_domain_coinbase_height_refused_even_if_asert_passes() {
    let height = 500_000_001;
    assert!(safa::asert_band_ok("chipnet", 0x0101_0000, block_time(), height).unwrap());
    refuses(&raw_claim(
        block_height() - 10,
        height,
        0x0101_0000,
        height,
        p2pkh(0x12),
    ));
}
#[test]
fn vault_must_keep_800_satoshis() {
    let sizing = default_scenario();
    let exact = sizing.claim.fee + 1600;
    let scenario = scenario(ScenarioOptions {
        value: exact,
        ..Default::default()
    })
    .unwrap();
    assert_eq!(scenario.claim.transaction.outputs[0].value_satoshis, 800);
    accepts(&scenario.claim);
    let mut below = scenario.claim.clone();
    below.transaction.outputs[0].value_satoshis -= 1;
    refuses(&below);
    assert!(
        test_utils::scenario(ScenarioOptions {
            value: exact - 1,
            ..Default::default()
        })
        .is_err()
    );
}
#[test]
fn payout_script_length_35_is_maximum() {
    let payout = [&[0xaa, 0x20][..], &[0x42; 32], &[0x87]].concat();
    assert_eq!(payout.len(), 35);
    let control = raw_claim(
        block_height() - 10,
        block_height(),
        block_bits(),
        block_height(),
        payout,
    );
    accepts(&control);
    assert!(nonstandard(&control).is_ok());
    let long = vec![0x51; 36];
    assert!(
        nonstandard(&raw_claim(
            block_height() - 10,
            block_height(),
            block_bits(),
            block_height(),
            long.clone()
        ))
        .is_err()
    );
    assert!(
        scenario(ScenarioOptions {
            payout: long.clone(),
            ..Default::default()
        })
        .is_err()
    );
    let mut split = split();
    split.donation_locking_bytecode = long;
    assert!(
        scenario(ScenarioOptions {
            split: Some(split),
            ..Default::default()
        })
        .is_err()
    );
}
#[test]
fn claim_fee_rate_one() {
    accepts(&default_scenario().claim);
}
#[test]
fn claim_fee_rate_two() {
    accepts(
        &scenario(ScenarioOptions {
            fee_rate: 2,
            ..Default::default()
        })
        .unwrap()
        .claim,
    );
}
#[test]
fn claim_fee_rate_three() {
    accepts(
        &scenario(ScenarioOptions {
            fee_rate: 3,
            ..Default::default()
        })
        .unwrap()
        .claim,
    );
}
#[test]
fn fee_under_bound_refused() {
    let mut claim = default_scenario().claim;
    claim.transaction.outputs[0].value_satoshis += 1;
    refuses(&claim);
}
#[test]
fn fee_over_three_times_bound_refused() {
    let mut claim = scenario(ScenarioOptions {
        fee_rate: 3,
        ..Default::default()
    })
    .unwrap()
    .claim;
    claim.transaction.outputs[0].value_satoshis -= 1;
    refuses(&claim);
}
#[test]
fn fee_rate_four_refused_by_builder() {
    assert!(
        scenario(ScenarioOptions {
            fee_rate: 4,
            ..Default::default()
        })
        .is_err()
    );
}
#[test]
fn thread_63_claims_last_thread_and_eases_target() {
    let mut options = ScenarioOptions {
        covenant: safa::create_covenant("chipnet", 63, 42_000_000_000).unwrap(),
        ..Default::default()
    };
    options.state.thread = 62;
    options.state.height = block_height() - 288;
    let scenario = scenario(options).unwrap();
    assert!(
        safa::expand_token_target(scenario.claim.plan.target_bits)
            .unwrap()
            .cmp(&safa::expand_token_target(0x2000_ffff).unwrap())
            .is_gt()
    );
    accepts(&scenario.claim);
}
#[test]
fn chipnet_64_threads_refused() {
    assert!(safa::create_covenant("chipnet", 64, 42_000_000_000).is_err());
}
#[test]
fn chipnet_288_threads_refused() {
    assert!(safa::create_covenant("chipnet", 288, 42_000_000_000).is_err());
}
#[test]
fn mainnet_64_threads_refused() {
    assert!(safa::create_covenant("mainnet", 64, 42_000_000_000).is_err());
}
macro_rules! easing_case {
    ($name:ident,$delta:expr) => {
        #[test]
        fn $name() {
            let mut options = ScenarioOptions::default();
            options.state.height = block_height() - $delta;
            options.state.target_bits = 0x1f00_ffff;
            let scenario = scenario(options).unwrap();
            accepts(&scenario.claim);
            let factor = number(($delta.min(288) * 5000 / 4 + 5000) as i64);
            let expected = safa::expand_token_target(0x1f00_ffff)
                .unwrap()
                .mul(&factor)
                .div_mod(&number(10000))
                .0;
            assert_eq!(
                scenario.claim.plan.target_bits,
                safa::encode_token_target(&expected).unwrap()
            );
        }
    };
}
easing_case!(target_eases_after_one, 1u32);
easing_case!(target_eases_after_four, 4u32);
easing_case!(target_eases_after_100, 100u32);
easing_case!(target_eases_after_288, 288u32);
easing_case!(target_easing_caps_at_288, 5000u32);
#[test]
fn leaf_cannot_claim_easier_target() {
    refuses(
        &scenario(ScenarioOptions {
            leaf_target_bits: Some(0x207f_ffff),
            ..Default::default()
        })
        .unwrap()
        .claim,
    );
}
#[test]
fn market_trade_preserves_product_and_claim_proof() {
    let scenario = default_scenario();
    let vault = &scenario.claim.source_outputs[0];
    let numerator = u128::from(vault.value_satoshis + scenario.covenant.virtual_sats);
    let cash = 1_000_000u64;
    let fair = token(vault).amount
        - ((numerator * u128::from(token(vault).amount) + numerator + u128::from(cash) - 1)
            / (numerator + u128::from(cash))) as u64;
    let valid = trade(vault, cash, fair);
    accepts(&valid);
    refuses(&trade(vault, cash, fair + 1));
    let traded = &valid.transaction.outputs[0];
    let claim = safa::build_claim(
        &scenario.covenant,
        &Outpoint {
            txid: safa::hash256(b"traded vault"),
            vout: 0,
        },
        &Vault {
            category: category(),
            value: traded.value_satoshis,
            token_amount: token(traded).amount,
            commitment: token(traded).nft.as_ref().unwrap().commitment.clone(),
        },
        &p2pkh(0x12),
        None,
        &scenario.proof,
        1,
        None,
    )
    .unwrap();
    accepts(&claim);
}
#[test]
fn deepest_proof_passes() {
    accepts(
        &scenario(ScenarioOptions {
            aux_height: Some(16),
            coinbase_depth: 25,
            split: Some(split()),
            ..Default::default()
        })
        .unwrap()
        .claim,
    );
}
#[test]
fn full_10000_byte_unlock_passes() {
    let scenario = full_unlock(10000).unwrap();
    assert_eq!(
        scenario.claim.transaction.inputs[0]
            .unlocking_bytecode
            .len(),
        10000
    );
    // The largest coinbase a claim carries at the deepest branches.
    let proof = safa::decode_proof(&scenario.proof).unwrap();
    assert_eq!(proof.head.len() + 53 + proof.tail.len(), 7173);
    accepts(&scenario.claim);
}
#[test]
fn unlocking_10001_bytes_refused() {
    assert!(full_unlock(10001).is_err());
}
#[test]
fn metrics_empty_branches() {
    let scenario = scenario(ScenarioOptions {
        other_tokens: 0,
        coinbase_depth: 0,
        ..Default::default()
    })
    .unwrap();
    metrics(&scenario.claim, (1595, 121_745, 1_308_800, 40, 818));
    assert!(scenario.covenant.redeem_script.len() < 1500);
}
#[test]
fn metrics_split() {
    metrics(
        &scenario(ScenarioOptions {
            covenant: one_sat(),
            split: Some(split()),
            ..Default::default()
        })
        .unwrap()
        .claim,
        (1729, 136_035, 1_416_000, 53, 885),
    );
}
#[test]
fn metrics_deepest() {
    metrics(
        &scenario(ScenarioOptions {
            aux_height: Some(16),
            coinbase_depth: 25,
            split: Some(split()),
            ..Default::default()
        })
        .unwrap()
        .claim,
        (2977, 257_638, 2_414_400, 153, 1509),
    );
}
#[test]
fn metrics_deepest_fallback() {
    let mut options = ScenarioOptions {
        aux_height: Some(16),
        coinbase_depth: 25,
        split: Some(split()),
        bits: 0x1b04_04cb,
        ..Default::default()
    };
    options.state.height = block_height() - 4032;
    options.state.ref_bits = 0x1b04_04cb;
    metrics(
        &scenario(options).unwrap().claim,
        (2977, 267_247, 2_414_400, 153, 1509),
    );
}
#[test]
fn metrics_full_unlock() {
    metrics(
        &full_unlock(10000).unwrap().claim,
        (10000, 370_058, 8_032_800, 263, 5020),
    );
}
