// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Reusable SAFA fixtures and BCH 2026 VM harness.
#![allow(dead_code)]

pub mod cli;
pub mod headers;
pub mod vectors;

use libauth::{
    AuthenticationProgramCommon, AuthenticationProgramStateBch2026,
    AuthenticationVirtualMachineBch2026, Input, Output, ResolvedTransactionCommon, Token,
    TransactionCommon, big_int_to_vm_number, create_virtual_machine_bch_2026, encode_data_push,
    encode_transaction, vm_number_to_big_int,
};
use p2poolv2_safa::{
    self as safa, BigInt, BuiltClaim, Covenant, Leaf, Outpoint, Proof, Split, State, TreeLeaf,
    Vault,
};

pub fn number(value: i64) -> BigInt {
    BigInt::from_i64(value)
}
pub fn stack_number(bytes: &[u8]) -> BigInt {
    vm_number_to_big_int(bytes, None, Some(true)).unwrap()
}
pub fn uint32(value: u32) -> Vec<u8> {
    value.to_le_bytes().to_vec()
}
pub fn p2pkh(byte: u8) -> Vec<u8> {
    [&[0x76, 0xa9, 0x14][..], &[byte; 20], &[0x88, 0xac]].concat()
}
pub fn covenant() -> Covenant {
    safa::create_covenant("chipnet", 4, 42_000_000_000).unwrap()
}
pub fn one_sat() -> Covenant {
    safa::create_covenant("chipnet", 4, 419_998_000_000).unwrap()
}
pub fn category() -> Vec<u8> {
    safa::hash256(b"tSAFA test category")
}
pub fn split() -> Split {
    Split {
        donation_bps: 100,
        donation_locking_bytecode: p2pkh(0xd0),
        fee_bps: 100,
        fee_locking_bytecode: p2pkh(0xfe),
    }
}
pub fn header_time(header: &str) -> u32 {
    u32::from_le_bytes(hex::decode(header).unwrap()[68..72].try_into().unwrap())
}
pub fn header_bits(header: &str) -> u32 {
    u32::from_le_bytes(hex::decode(header).unwrap()[72..76].try_into().unwrap())
}
pub fn block_height() -> u32 {
    headers::CHIPNET_PAIRS[8].0 + 1
}
pub fn block_time() -> u32 {
    header_time(headers::CHIPNET_PAIRS[8].2)
}
pub fn block_bits() -> u32 {
    header_bits(headers::CHIPNET_PAIRS[8].2)
}

pub fn bare_transaction() -> TransactionCommon {
    TransactionCommon {
        version: 2,
        locktime: 0,
        inputs: vec![Input {
            outpoint_transaction_hash: vec![0; 32],
            outpoint_index: 0,
            sequence_number: 0,
            unlocking_bytecode: vec![],
        }],
        outputs: vec![Output {
            locking_bytecode: [&[0x6a][..], &[0; 29]].concat(),
            value_satoshis: 0,
            token: None,
        }],
    }
}

pub struct FunctionCaller {
    library: Vec<u8>,
    machine: AuthenticationVirtualMachineBch2026,
}
impl FunctionCaller {
    pub fn new(network: &str) -> Self {
        Self {
            library: safa::compile_library(network, "").unwrap(),
            machine: create_virtual_machine_bch_2026(Some(false)),
        }
    }
    pub fn call(
        &self,
        identifier: u64,
        arguments: &[Vec<u8>],
    ) -> AuthenticationProgramStateBch2026 {
        let mut script = self.library.clone();
        for argument in arguments {
            script.extend(encode_data_push(argument));
        }
        script.extend(encode_data_push(&big_int_to_vm_number(&BigInt::from_u64(
            identifier,
        ))));
        script.push(0x8a);
        let mut state = self.machine.evaluate(
            &AuthenticationProgramCommon {
                input_index: 0,
                transaction: bare_transaction(),
                source_outputs: vec![Output {
                    locking_bytecode: script,
                    value_satoshis: 0,
                    token: None,
                }],
            },
            None,
        );
        if state
            .error
            .as_ref()
            .is_some_and(|error| error.contains("unexpected number of items on the stack"))
        {
            state.error = None;
        }
        state
    }
}
pub fn numeric(value: i64) -> Vec<u8> {
    big_int_to_vm_number(&number(value))
}
pub fn big_numeric(value: &BigInt) -> Vec<u8> {
    big_int_to_vm_number(value)
}
pub fn ok(state: AuthenticationProgramStateBch2026) -> Vec<Vec<u8>> {
    assert!(state.error.is_none(), "{:?}", state.error);
    state.stack
}
pub fn fails(state: AuthenticationProgramStateBch2026) {
    assert!(
        state.error.is_some(),
        "the covenant accepted the invalid case"
    );
}
pub fn run_functions(network: &str, body: &str) -> AuthenticationProgramStateBch2026 {
    create_virtual_machine_bch_2026(Some(false)).evaluate(
        &AuthenticationProgramCommon {
            input_index: 0,
            transaction: bare_transaction(),
            source_outputs: vec![Output {
                locking_bytecode: safa::compile_library(network, body).unwrap(),
                value_satoshis: 0,
                token: None,
            }],
        },
        None,
    )
}
pub fn accepts(claim: &BuiltClaim) {
    safa::verify_transaction(&claim.transaction, &claim.source_outputs).unwrap();
}
pub fn refuses(claim: &BuiltClaim) {
    assert!(
        safa::verify_transaction(&claim.transaction, &claim.source_outputs).is_err(),
        "the covenant accepted the invalid spend"
    );
}
pub fn nonstandard(claim: &BuiltClaim) -> Result<(), String> {
    create_virtual_machine_bch_2026(Some(false)).verify(&ResolvedTransactionCommon {
        transaction: claim.transaction.clone(),
        source_outputs: claim.source_outputs.clone(),
    })
}
/// Check a claim's VM metrics against the values the Node tool measured with
/// JavaScript Libauth: unlocking bytes, operation cost and its limit, hash
/// iterations and their limit.
pub fn metrics(claim: &BuiltClaim, expected: (usize, u64, u64, u64, u64)) {
    accepts(claim);
    let state = create_virtual_machine_bch_2026(Some(true)).evaluate(
        &AuthenticationProgramCommon {
            input_index: 0,
            transaction: claim.transaction.clone(),
            source_outputs: claim.source_outputs.clone(),
        },
        None,
    );
    assert!(state.error.is_none(), "{:?}", state.error);
    assert!(state.metrics.operation_cost <= state.metrics.maximum_operation_cost);
    assert!(state.metrics.hash_digest_iterations <= state.metrics.maximum_hash_digest_iterations);
    assert!(claim.size <= claim.plan.size_bound);
    let measured = (
        claim.transaction.inputs[0].unlocking_bytecode.len(),
        state.metrics.operation_cost as u64,
        state.metrics.maximum_operation_cost as u64,
        state.metrics.hash_digest_iterations as u64,
        state.metrics.maximum_hash_digest_iterations as u64,
    );
    eprintln!("unlock, operation cost and limit, hash iterations and limit: {measured:?}");
    assert_eq!(measured, expected);
}
pub fn set_unlock(
    claim: &mut BuiltClaim,
    covenant: &Covenant,
    preimage: &[u8],
    payout_hash: &[u8],
    proof: &[u8],
) {
    claim.transaction.inputs[0].unlocking_bytecode =
        safa::claim_unlocking_bytecode(covenant, preimage, payout_hash, proof).unwrap();
}
pub fn token(output: &Output) -> &Token {
    output.token.as_ref().unwrap()
}
pub fn token_mut(output: &mut Output) -> &mut Token {
    output.token.as_mut().unwrap()
}
pub fn commitment_mut(output: &mut Output) -> &mut Vec<u8> {
    &mut token_mut(output).nft.as_mut().unwrap().commitment
}

pub fn compact_size(value: usize) -> Vec<u8> {
    if value < 253 {
        vec![value as u8]
    } else if value <= 65535 {
        [&[0xfd][..], &(value as u16).to_le_bytes()].concat()
    } else {
        [&[0xfe][..], &(value as u32).to_le_bytes()].concat()
    }
}
pub struct Coinbase {
    pub head: Vec<u8>,
    pub tail: Vec<u8>,
    pub coinbase: Vec<u8>,
}
pub fn make_coinbase(
    height: u32,
    output0: &[u8],
    payouts: &[(u64, Vec<u8>)],
    height_push: Option<&[u8]>,
) -> Coinbase {
    let encoded = encode_data_push(&big_int_to_vm_number(&BigInt::from_u64(height as u64)));
    let script = [height_push.unwrap_or(&encoded), &[7; 8]].concat();
    let head = [
        &2u32.to_le_bytes()[..],
        &[1],
        &[0; 32],
        &u32::MAX.to_le_bytes(),
        &[script.len() as u8],
        &script,
        &u32::MAX.to_le_bytes(),
        &[(1 + payouts.len()) as u8],
    ]
    .concat();
    let mut tail = Vec::with_capacity(payouts.len() * 40 + 4);
    for (value, script) in payouts {
        tail.extend(value.to_le_bytes());
        tail.extend(compact_size(script.len()));
        tail.extend(script);
    }
    tail.extend(0u32.to_le_bytes());
    let coinbase = [head.as_slice(), output0, tail.as_slice()].concat();
    Coinbase {
        head,
        tail,
        coinbase,
    }
}
pub fn mine_header(merkle_root: &[u8], time: u32, bits: u32, target: &BigInt) -> Vec<u8> {
    let mut header = [
        &0x2000_0000u32.to_le_bytes()[..],
        &[0x55; 32],
        merkle_root,
        &time.to_le_bytes(),
        &bits.to_le_bytes(),
        &[0; 4],
    ]
    .concat();
    let mut nonce = 0u32;
    while nonce < (1 << 24) {
        header[76..80].copy_from_slice(&nonce.to_le_bytes());
        if BigInt::from_bytes_le(&safa::hash256(&header))
            .cmp(target)
            .is_le()
        {
            return header;
        }
        nonce += 1;
    }
    panic!("test mining exceeded its budget")
}
pub fn mine_existing_header(header: &mut [u8], target: &BigInt) {
    assert_eq!(header.len(), 80);
    let mut nonce = 0u32;
    while nonce < (1 << 24) {
        header[76..80].copy_from_slice(&nonce.to_le_bytes());
        if BigInt::from_bytes_le(&safa::hash256(header))
            .cmp(target)
            .is_le()
        {
            return;
        }
        nonce += 1;
    }
    panic!("fixture mining exceeded its budget")
}

#[derive(Clone)]
pub struct ScenarioOptions {
    pub covenant: Covenant,
    pub state: State,
    pub value: u64,
    pub token_amount: u64,
    pub category: Vec<u8>,
    pub payout: Vec<u8>,
    pub split: Option<Split>,
    pub height: u32,
    pub time: u32,
    pub bits: u32,
    pub other_tokens: usize,
    pub aux_height: Option<u8>,
    pub coinbase_depth: usize,
    pub payouts: Vec<(u64, Vec<u8>)>,
    pub fee_rate: u32,
    pub locktime: Option<u32>,
    pub leaf_target_bits: Option<u32>,
}
impl Default for ScenarioOptions {
    fn default() -> Self {
        Self {
            covenant: covenant(),
            state: State {
                thread: 1,
                height: block_height() - 10,
                target_bits: 0x2000_ffff,
                ref_bits: block_bits(),
            },
            value: 2_000_000,
            token_amount: 420_000_000_000,
            category: category(),
            payout: p2pkh(0x12),
            split: None,
            height: block_height(),
            time: block_time(),
            bits: block_bits(),
            other_tokens: 1,
            aux_height: None,
            coinbase_depth: 1,
            payouts: vec![(312_500_000, p2pkh(0x12))],
            fee_rate: 1,
            locktime: None,
            leaf_target_bits: None,
        }
    }
}
pub struct Scenario {
    pub covenant: Covenant,
    pub vault: Vault,
    pub proof: Vec<u8>,
    pub claim: BuiltClaim,
}
pub fn scenario(options: ScenarioOptions) -> Result<Scenario, String> {
    let vault = Vault {
        category: options.category.clone(),
        value: options.value,
        token_amount: options.token_amount,
        commitment: safa::encode_state(&options.state)?,
    };
    let mut leaf = safa::claim_leaf(
        &options.covenant,
        &vault,
        &options.payout,
        options.split.as_ref(),
        options.height,
    )?;
    if let Some(bits) = options.leaf_target_bits {
        leaf.leaf.target_bits = bits;
        leaf.target_bits = bits;
        leaf.leaf_hash = safa::leaf_hash(&leaf.leaf)?;
    }
    let (root, height, salt, branch) = if let Some(height) = options.aux_height {
        let salt = 7;
        let branch: Vec<Vec<u8>> = (0..height)
            .map(|level| safa::hash256(&[0xaa, level]))
            .collect();
        (
            safa::fold_aux(
                &leaf.leaf_hash,
                safa::slot(&leaf.leaf.category, salt, height),
                &branch,
            ),
            height,
            salt,
            branch,
        )
    } else {
        let mut leaves = Vec::with_capacity(options.other_tokens + 1);
        leaves.push(TreeLeaf {
            category: leaf.leaf.category.clone(),
            hash: leaf.leaf_hash.clone(),
        });
        leaves.extend((0..options.other_tokens).map(|index| TreeLeaf {
            category: vec![0x30 + index as u8; 32],
            hash: safa::hash256(&[0xa0, index as u8]),
        }));
        let tree = safa::build_tree(&leaves)?;
        (tree.root, tree.height, tree.salt, tree.branches[0].clone())
    };
    let coinbase = make_coinbase(
        options.height,
        &safa::commitment_output(&root, height, salt),
        &options.payouts,
        None,
    );
    let coinbase_branch: Vec<Vec<u8>> = (0..options.coinbase_depth)
        .map(|level| safa::hash256(&[0xbb, level as u8]))
        .collect();
    let root = safa::coinbase_merkle_root(&coinbase.coinbase, &coinbase_branch);
    let header = mine_header(
        &root,
        options.time,
        options.bits,
        &safa::expand_token_target(leaf.target_bits)?,
    );
    let proof = safa::encode_proof(&Proof {
        header,
        height,
        salt,
        aux_branch: branch,
        head: coinbase.head,
        tail: coinbase.tail,
        coinbase_branch,
    })?;
    let claim = safa::build_claim(
        &options.covenant,
        &Outpoint {
            txid: safa::hash256(b"vault outpoint"),
            vout: 0,
        },
        &vault,
        &options.payout,
        options.split.as_ref(),
        &proof,
        options.fee_rate,
        options.locktime,
    )?;
    Ok(Scenario {
        covenant: options.covenant,
        vault,
        proof,
        claim,
    })
}
pub fn default_scenario() -> Scenario {
    scenario(ScenarioOptions::default()).unwrap()
}
pub fn full_unlock(size: usize) -> Result<Scenario, String> {
    let first = scenario(ScenarioOptions {
        aux_height: Some(16),
        coinbase_depth: 25,
        split: Some(split()),
        payouts: vec![(312_500_000, p2pkh(0x12)), (0, vec![0x51; 6000])],
        ..Default::default()
    })?;
    scenario(ScenarioOptions {
        aux_height: Some(16),
        coinbase_depth: 25,
        split: Some(split()),
        payouts: vec![
            (312_500_000, p2pkh(0x12)),
            (
                0,
                vec![
                    0x51;
                    6000 + size - first.claim.transaction.inputs[0].unlocking_bytecode.len()
                ],
            ),
        ],
        ..Default::default()
    })
}
pub fn raw_claim(
    state_height: u32,
    height: u32,
    bits: u32,
    locktime: u32,
    payout: Vec<u8>,
) -> BuiltClaim {
    let mut claim = default_scenario().claim;
    let covenant = covenant();
    let commitment = safa::encode_state(&State {
        thread: 1,
        height: state_height,
        target_bits: 0x2000_ffff,
        ref_bits: block_bits(),
    })
    .unwrap();
    let target_bits =
        safa::claim_target_bits(0x2000_ffff, height.saturating_sub(state_height).max(1), 4)
            .unwrap();
    let mut wire_category = category();
    wire_category.reverse();
    let leaf = Leaf {
        category: wire_category,
        lock_hash: covenant.lock_hash.clone(),
        state_hash: safa::hash256(&commitment),
        payout_hash: safa::hash256(&payout),
        target_bits,
        ext: vec![0; 32],
    };
    let coinbase = make_coinbase(
        height,
        &safa::commitment_output(&safa::leaf_hash(&leaf).unwrap(), 0, 0),
        &[(312_500_000, p2pkh(0x12))],
        None,
    );
    let header = mine_header(
        &safa::hash256(&coinbase.coinbase),
        block_time(),
        bits,
        &safa::expand_token_target(target_bits).unwrap(),
    );
    let proof = safa::encode_proof(&Proof {
        header: header.clone(),
        height: 0,
        salt: 0,
        aux_branch: vec![],
        head: coinbase.head,
        tail: coinbase.tail,
        coinbase_branch: vec![],
    })
    .unwrap();
    let fee = (covenant.fee_k + proof.len()) as u64;
    let next = [
        &commitment[..4],
        &height.to_le_bytes(),
        &target_bits.to_le_bytes(),
        &header[72..76],
    ]
    .concat();
    claim.source_outputs[0]
        .token
        .as_mut()
        .unwrap()
        .nft
        .as_mut()
        .unwrap()
        .commitment = commitment;
    claim.transaction.locktime = locktime;
    claim.transaction.outputs.truncate(2);
    claim.transaction.outputs[0].value_satoshis = 2_000_000 - fee - 800;
    *commitment_mut(&mut claim.transaction.outputs[0]) = next;
    claim.transaction.outputs[1].locking_bytecode = payout;
    set_unlock(&mut claim, &covenant, &[], &leaf.payout_hash, &proof);
    claim.size = encode_transaction(&claim.transaction).len();
    claim
}

pub fn trade(vault: &Output, cash_in: u64, tokens_out: u64) -> BuiltClaim {
    let mut claim = default_scenario().claim;
    let mut next = vault.clone();
    next.value_satoshis += cash_in;
    token_mut(&mut next).amount -= tokens_out;
    claim.source_outputs = vec![
        vault.clone(),
        Output {
            locking_bytecode: vec![0x51],
            value_satoshis: cash_in + 2000,
            token: None,
        },
    ];
    claim.transaction = TransactionCommon {
        version: 2,
        locktime: 0,
        inputs: vec![
            Input {
                outpoint_transaction_hash: safa::hash256(b"vault outpoint"),
                outpoint_index: 0,
                sequence_number: u32::MAX,
                unlocking_bytecode: safa::market_unlocking_bytecode(&covenant()),
            },
            Input {
                outpoint_transaction_hash: safa::hash256(b"buyer"),
                outpoint_index: 0,
                sequence_number: u32::MAX,
                unlocking_bytecode: vec![],
            },
        ],
        outputs: vec![
            next,
            Output {
                locking_bytecode: p2pkh(0x77),
                value_satoshis: 1000,
                token: Some(Token {
                    category: token(vault).category.clone(),
                    amount: tokens_out,
                    nft: None,
                }),
            },
        ],
    };
    claim
}

pub fn check_asert_run(run: &str) -> usize {
    let parameter = |name: &str| {
        run.lines()
            .find_map(|line| line.strip_prefix(&format!("##   {name}: ")))
            .unwrap()
            .trim()
    };
    // run10 crosses the signed 64-bit height limit, so heights parse wide.
    let anchor_height: i128 = parameter("anchor height").parse().unwrap();
    let anchor_time: i64 = parameter("anchor parent time").parse().unwrap();
    let anchor_bits =
        u32::from_str_radix(parameter("anchor nBits").trim_start_matches("0x"), 16).unwrap();
    let iterations: usize = parameter("iterations").parse().unwrap();
    let reference = safa::set_compact(anchor_bits).unwrap();
    let caller = FunctionCaller::new("mainnet");
    let mut count = 0;
    for line in run
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
    {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let height: i128 = fields[1].parse().unwrap();
        let height_diff = i64::try_from(height - anchor_height).unwrap();
        let time: i64 = fields[2].parse().unwrap();
        let bits = u32::from_str_radix(fields[3].trim_start_matches("0x"), 16).unwrap();
        let result = ok(caller.call(
            6,
            &[
                big_numeric(&reference),
                numeric(time - anchor_time),
                numeric(height_diff),
                numeric(172800),
            ],
        ));
        let target = stack_number(&result[0]);
        assert_eq!(safa::get_compact(&target), bits, "{line}");
        assert_eq!(
            target,
            safa::calculate_asert(&reference, time - anchor_time, height_diff, 172800),
            "{line}"
        );
        count += 1;
    }
    assert_eq!(count, iterations);
    count
}
pub fn check_header_pairs(network: &str, pairs: &[(u32, &str, &str)]) {
    assert_eq!(pairs.len(), 9);
    for (height, parent, child) in pairs {
        assert!(band(
            network,
            header_bits(child),
            header_time(child),
            height + 1
        ));
        let target = safa::asert_target(network, height + 1, header_time(parent))
            .unwrap()
            .unwrap();
        assert!(
            safa::get_compact(&target) == header_bits(child)
                || (network == "chipnet" && header_bits(child) == 0x1d00_ffff)
        );
    }
}
pub fn band(network: &str, bits: u32, time: u32, height: u32) -> bool {
    let result =
        ok(FunctionCaller::new(network)
            .call(7, &[uint32(bits), uint32(time), numeric(height as i64)]));
    let accepted = stack_number(&result[0]) == number(1);
    assert_eq!(
        accepted,
        safa::asert_band_ok(network, bits, time, height).unwrap()
    );
    accepted
}
pub fn check_band_matrix() {
    let pair = headers::MAINNET_PAIRS[8];
    for offset in [-86400i64 * 30, -3600, 0, 7200, 86400 * 30, 86400 * 400] {
        for bits in [
            header_bits(pair.2),
            0x1801_0000,
            0x1803_ffff,
            0x1810_0000,
            0x1c00_ffff,
            0x1703_0ecd,
        ] {
            band(
                "mainnet",
                bits,
                (i64::from(header_time(pair.2)) + offset) as u32,
                pair.0 + 1,
            );
        }
    }
}
pub fn check_target_rounding() {
    let caller = FunctionCaller::new("chipnet");
    for target in [
        number(1),
        number(0x7fffff),
        number(0x800000),
        number(0x123456789),
        safa::max_token_target(),
        number(1).shl(200).add(&number(12345)),
    ] {
        let bits = ok(caller.call(4, &[big_numeric(&target)]))[0].clone();
        let encoded = safa::encode_token_target(&target).unwrap();
        assert_eq!(bits, uint32(encoded));
        assert!(
            safa::expand_token_target(encoded)
                .unwrap()
                .cmp(&target)
                .is_le()
        );
    }
}
pub fn check_coinbase_shapes() {
    let good = make_coinbase(
        1,
        &safa::commitment_output(&[0; 32], 0, 0),
        &[(312_500_000, p2pkh(0x12))],
        Some(&hex::decode("0315f904").unwrap()),
    )
    .head;
    let caller = FunctionCaller::new("chipnet");
    for (offset, value) in [(4, 2), (good.len() - 1, 0), (good.len() - 1, 0xfd)] {
        let mut changed = good.clone();
        changed[offset] = value;
        fails(caller.call(2, &[changed.clone()]));
        assert!(safa::coinbase_height(&changed).is_err());
    }
    fails(caller.call(2, &[good[..good.len() - 1].to_vec()]));
}
pub fn proof_body(proof: &[u8], leaf: &[u8], category: &[u8], bits: u32) -> String {
    format!(
        "<0x{}> <1> OP_INVOKE <0x{}> <0x{}> <6> OP_PICK <8> OP_PICK <9> OP_INVOKE <5> OP_ROLL <10> OP_INVOKE <5> OP_ROLL <5> OP_ROLL <11> OP_INVOKE <3> OP_ROLL OP_SWAP <3> OP_ROLL <3> OP_ROLL <12> OP_INVOKE OP_OVER <36> OP_SPLIT OP_NIP <32> OP_SPLIT OP_DROP OP_EQUALVERIFY <0x{}> <3> OP_INVOKE <13> OP_INVOKE OP_1",
        hex::encode(proof),
        hex::encode(leaf),
        hex::encode(category),
        hex::encode(bits.to_le_bytes())
    )
}
