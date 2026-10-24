// Whole claims and market spends of the chipnet covenant in the standard BCH
// 2026 VM, the attacks that must fail, and the worst-case VM cost.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { encodeTransactionBch, flattenBinArray, hash256, hexToBin } from '@bitauth/libauth';
import { mainnetPairs } from './rust_vectors.mjs';
import * as safa from './lib.mjs';
import {
  CATEGORY, CHIPNET_BLOCK, COVENANT, claimScenario, encodeHeightPush, inputMetrics, makeCoinbase, mineHeader,
  p2pkh, setUnlock, utf8, verify, verifyNonstandard,
} from './test_utils.mjs';

const accepts = (spend, why) => assert.equal(verify(spend), true, why);
const refuses = (spend, why) => assert.notEqual(verify(spend), true, why ?? 'the covenant accepted it');
const clone = claim => ({ transaction: structuredClone(claim.transaction), sourceOutputs: structuredClone(claim.sourceOutputs) });
const SPLIT = Object.freeze({
  donationBps: 100, donationLockingBytecode: p2pkh('d0'), feeBps: 100, feeLockingBytecode: p2pkh('fe'),
});
/** One satoshi per token: virtual + cash = 420,000,000,000 sats for as many tokens. */
const ONE_SAT = safa.createCovenant({ network: 'chipnet', threads: 4, virtualSats: 419998000000n });

test('a claim without a split pays the miner the whole reward, at 1 sat/byte of its bound', () => {
  const { claim, vault } = claimScenario();
  accepts(claim);
  const [continuing, miner] = claim.transaction.outputs;
  assert.equal(claim.transaction.outputs.length, 2);
  assert.equal(miner.token.amount, 1000000n);
  assert.equal(miner.valueSatoshis, 800n);
  assert.equal(continuing.token.amount, vault.tokenAmount - 1000000n);
  assert.equal(continuing.valueSatoshis, vault.value - claim.fee - 800n);
  assert.equal(claim.fee, BigInt(claim.plan.sizeBound));
  assert.ok(claim.size <= claim.plan.sizeBound, `${claim.size} bytes within the bound ${claim.plan.sizeBound}`);
  assert.deepEqual(safa.decodeState(continuing.token.nft.commitment), {
    thread: 1, height: CHIPNET_BLOCK.height, targetBits: claim.plan.targetBits, refBits: CHIPNET_BLOCK.bits,
  });
});

test('the golden split claim pays 980,100 / 10,000 / 9,900 at one satoshi per token', () => {
  const { claim } = claimScenario({ covenant: ONE_SAT, split: SPLIT });
  accepts(claim);
  assert.deepEqual(claim.transaction.outputs.slice(1).map(output => [output.token.amount, output.valueSatoshis]), [
    [980100n, 800n], [10000n, 800n], [9900n, 800n],
  ]);
  assert.deepEqual(claim.transaction.outputs.slice(1).map(output => output.lockingBytecode),
    [p2pkh('12'), p2pkh('d0'), p2pkh('fe')]);
  assert.ok(claim.size <= claim.plan.sizeBound);
});

test('a whole donation leaves no miner output, and the payout hash still binds the leaf', () => {
  const split = { ...SPLIT, donationBps: 10000, feeBps: 0, feeLockingBytecode: undefined };
  const scenario = claimScenario({ covenant: ONE_SAT, split });
  accepts(scenario.claim);
  assert.deepEqual(scenario.claim.transaction.outputs.slice(1).map(output => output.token.amount), [1000000n]);
  const other = clone(scenario.claim);
  setUnlock(other, ONE_SAT, { ...scenario.claim.plan, payoutHash: hash256(p2pkh('99')) });
  refuses(other);
});

test('a donation worth under 1,000 satoshis stays with the miner, and the fee does not grow', () => {
  const split = { ...SPLIT, donationBps: 99, feeBps: 200 };
  const { claim } = claimScenario({ split });
  accepts(claim);
  // 9,900 tokens are worth 990 satoshis at 42,002,000,000 / 420,000,000,000.
  assert.deepEqual(claim.plan.shares, { miner: 980198n, donation: 0n, fee: 19802n });
  assert.deepEqual(claim.transaction.outputs.slice(1).map(output => output.lockingBytecode), [p2pkh('12'), p2pkh('fe')]);
  const paid = clone(claim);
  paid.transaction.outputs.splice(2, 0, { lockingBytecode: p2pkh('d0'), valueSatoshis: 800n, token: { category: CATEGORY, amount: 9900n } });
  paid.transaction.outputs[1].token.amount -= 9900n;
  paid.transaction.outputs[0].valueSatoshis -= 800n;
  refuses(paid, 'a donation below the minimum was paid');
  const grown = clone(claim);
  grown.transaction.outputs[1].token.amount -= 9900n;
  grown.transaction.outputs[2].token.amount += 9900n;
  refuses(grown, 'the fee took the dropped donation');
});

// ---- theft ----

test('theft: the miner output cannot be redirected, with or without a matching payout hash', () => {
  const { claim } = claimScenario({ split: SPLIT });
  const redirected = clone(claim);
  redirected.transaction.outputs[1].lockingBytecode = p2pkh('99');
  refuses(redirected);
  setUnlock(redirected, COVENANT, { ...claim.plan, payoutHash: hash256(p2pkh('99')) });
  refuses(redirected);
});

test('theft: no extra tokens, satoshis, outputs or inputs leave the vault', () => {
  const { claim } = claimScenario({ split: SPLIT });
  const tokens = clone(claim);
  tokens.transaction.outputs[0].token.amount -= 1n;
  tokens.transaction.outputs[1].token.amount += 1n;
  refuses(tokens, 'one token more for the miner');
  const sats = clone(claim);
  sats.transaction.outputs[0].valueSatoshis -= 1n;
  sats.transaction.outputs[1].valueSatoshis += 1n;
  refuses(sats, 'one satoshi more on the miner output');
  const extra = clone(claim);
  extra.transaction.outputs.push({ lockingBytecode: flattenBinArray([Uint8Array.of(0x6a), utf8('x')]), valueSatoshis: 0n });
  refuses(extra, 'an extra output');
  const sponsored = clone(claim);
  sponsored.sourceOutputs.push({ lockingBytecode: Uint8Array.of(0x51), valueSatoshis: 100000n });
  sponsored.transaction.inputs.push({
    outpointTransactionHash: hash256(utf8('sponsor')), outpointIndex: 0, sequenceNumber: 0xfffffffe,
    unlockingBytecode: new Uint8Array(),
  });
  refuses(sponsored, 'a second input');
});

test('theft: the vault keeps its covenant, category and mutable NFT', () => {
  const { claim } = claimScenario();
  const moved = clone(claim);
  moved.transaction.outputs[0].lockingBytecode = p2pkh('99');
  refuses(moved, 'another locking bytecode');
  const immutable = clone(claim);
  immutable.transaction.outputs[0].token.nft.capability = 'none';
  refuses(immutable, 'an immutable NFT');
  const swapped = clone(claim);
  [swapped.transaction.outputs[0].token, swapped.transaction.outputs[1].token] =
    [swapped.transaction.outputs[1].token, swapped.transaction.outputs[0].token];
  refuses(swapped, 'the NFT on the miner output');
});

test('theft: a proof cannot claim another thread or another category', () => {
  const { claim, proof } = claimScenario();
  const otherThread = clone(claim);
  otherThread.sourceOutputs[0].token.nft.commitment = safa.encodeState({
    thread: 2, height: CHIPNET_BLOCK.height - 10, targetBits: 0x2000ffff, refBits: CHIPNET_BLOCK.bits,
  });
  otherThread.transaction.outputs[0].token.nft.commitment[0] = 2;
  refuses(otherThread, 'thread 1 proof against thread 2');
  const foreign = hash256(utf8('another category'));
  const otherCategory = clone(claim);
  otherCategory.sourceOutputs[0].token.category = foreign;
  for (const output of otherCategory.transaction.outputs) output.token.category = foreign;
  refuses(otherCategory, 'replayed on another category');
  assert.ok(proof.length > 0);
});

// ---- the split ----

test('a wrong split fails: recipients, order, amounts, satoshis, category or a hidden preimage', () => {
  const split = { ...SPLIT, feeBps: 200 };
  const { claim } = claimScenario({ covenant: ONE_SAT, split });
  accepts(claim);
  const edits = {
    'donation to another script': tx => { tx.outputs[2].lockingBytecode = p2pkh('99'); },
    'donation and fee swapped': tx => { [tx.outputs[2], tx.outputs[3]] = [tx.outputs[3], tx.outputs[2]]; },
    'donation kept by the miner': tx => {
      tx.outputs[1].token.amount += tx.outputs[2].token.amount;
      tx.outputs.splice(2, 1);
      tx.outputs[0].valueSatoshis += 800n;
    },
    'one token moved from the fee to the donation': tx => {
      tx.outputs[2].token.amount += 1n;
      tx.outputs[3].token.amount -= 1n;
    },
    '801 satoshis on the donation': tx => {
      tx.outputs[2].valueSatoshis += 1n;
      tx.outputs[0].valueSatoshis -= 1n;
    },
  };
  for (const [why, edit] of Object.entries(edits)) {
    const changed = clone(claim);
    edit(changed.transaction);
    refuses(changed, why);
  }
  const hidden = clone(claim);
  setUnlock(hidden, ONE_SAT, { ...claim.plan, preimage: new Uint8Array() });
  refuses(hidden, 'the split preimage withheld');
  const rewritten = clone(claim);
  const preimage = safa.splitPreimage({
    donationBps: 100, donationHash: hash256(p2pkh('99')), feeBps: 200, feeHash: hash256(p2pkh('fe')),
  });
  rewritten.transaction.outputs[2].lockingBytecode = p2pkh('99');
  setUnlock(rewritten, ONE_SAT, { ...claim.plan, preimage });
  refuses(rewritten, 'a rewritten split with its own ext');
});

test('split outputs carry only the vault category, as fungible tokens', () => {
  const { claim } = claimScenario({ covenant: ONE_SAT, split: SPLIT });
  const sponsoredCategory = hash256(utf8('foreign tokens'));
  const changed = clone(claim);
  changed.transaction.outputs[3].token.category = sponsoredCategory;
  refuses(changed, 'the fee paid in another category');
});

// ---- BCH-only work ----

test('a Bitcoin header fails the ASERT band even with valid token work', () => {
  accepts(claimScenario().claim, 'the same claim with BCH nBits');
  refuses(claimScenario({ bits: 0x17030ecd }).claim, 'BTC nBits');
  refuses(claimScenario({ bits: 0x1d00fffe }).claim, 'almost the limit, but not the limit');
  accepts(claimScenario({ bits: safa.POW_LIMIT_BITS }).claim, 'chipnet allows a minimum-difficulty block');
});

test('the DAA-change fallback opens at 4,032 blocks, from ref >> 4 to ref x 1.1', () => {
  // 0x1b0404cb is exactly 0x1b404cb0 >> 4, and both fail the ASERT band here.
  const reference = 0x1b404cb0;
  const sixteenth = 0x1b0404cb;
  for (const bits of [reference, sixteenth, sixteenth - 1]) {
    assert.equal(safa.asertBandOk('chipnet', bits, CHIPNET_BLOCK.time, CHIPNET_BLOCK.height), false);
  }
  const at = (delta, bits, refBits = reference) => claimScenario({
    bits, state: { height: CHIPNET_BLOCK.height - delta, refBits },
  }).claim;
  accepts(at(4032, sixteenth), 'exactly ref >> 4 at 4,032 blocks');
  accepts(at(4032, reference), 'the reference itself');
  refuses(at(4031, sixteenth), 'one block too early');
  refuses(at(4031, reference), 'the reference one block too early');
  refuses(at(4032, sixteenth - 1), 'just harder than ref >> 4');
  refuses(at(100000, sixteenth - 1), 'no wider after more blocks');
  refuses(at(4032, sixteenth, CHIPNET_BLOCK.bits), 'far easier than the reference');
});

test('the mainnet covenant claims at a real mainnet height and allows no minimum-difficulty nBits', () => {
  const mainnet = safa.createCovenant({ network: 'mainnet', threads: 4, virtualSats: 42000000000n });
  const real = hexToBin(mainnetPairs.at(-1).child);
  const at = bits => claimScenario({
    covenant: mainnet, height: mainnetPairs.at(-1).parentHeight + 1, time: safa.readUint32(real, 68), bits,
    state: { height: mainnetPairs.at(-1).parentHeight - 9, refBits: safa.readUint32(real, 72) },
  }).claim;
  accepts(at(safa.readUint32(real, 72)), 'real mainnet nBits');
  refuses(at(safa.POW_LIMIT_BITS), 'powLimit on mainnet');
  refuses(at(0x17030ecd), 'BTC nBits on mainnet');
});

// ---- heights ----

test('a reused or older height fails, and a claimed proof cannot be replayed', () => {
  refuses(rawClaim({ stateHeight: CHIPNET_BLOCK.height }), 'the state height itself');
  refuses(rawClaim({ stateHeight: CHIPNET_BLOCK.height + 5 }), 'an older coinbase');
  accepts(rawClaim({ stateHeight: CHIPNET_BLOCK.height - 1 }), 'one block newer');

  const first = claimScenario();
  accepts(first.claim);
  const vault = first.claim.transaction.outputs[0];
  const next = claimScenario({
    state: safa.decodeState(vault.token.nft.commitment), value: vault.valueSatoshis, tokenAmount: vault.token.amount,
    height: CHIPNET_BLOCK.height + 1, time: CHIPNET_BLOCK.time + 600,
  });
  accepts(next.claim, 'the next height claims the new state');
  const replay = clone(first.claim);
  replay.sourceOutputs = [structuredClone(vault)];
  replay.transaction.outputs[0].token.amount = vault.token.amount - 1000000n;
  replay.transaction.outputs[0].valueSatoshis = vault.valueSatoshis - first.claim.fee - 800n;
  refuses(replay, 'the same proof again');
});

test('CLTV binds the coinbase height as a height-type lock time', () => {
  const { claim } = claimScenario();
  const early = clone(claim);
  early.transaction.locktime = CHIPNET_BLOCK.height - 1;
  refuses(early, 'locktime below the coinbase height');
  const later = clone(claim);
  later.transaction.locktime = CHIPNET_BLOCK.height + 100;
  accepts(later, 'a later height');
  const time = clone(claim);
  time.transaction.locktime = 1700000000;
  refuses(time, 'a time lock');
  const final = clone(claim);
  final.transaction.inputs[0].sequenceNumber = 0xffffffff;
  refuses(final, 'a final sequence disables CLTV');
});

test('a coinbase height in the time domain is refused although ASERT alone would pass it', () => {
  const height = safa.LOCKTIME_THRESHOLD + 1;
  // At that height ASERT's computed target is 1, so nBits for target 1 pass the band.
  assert.equal(safa.asertBandOk('chipnet', 0x01010000, CHIPNET_BLOCK.time, height), true);
  refuses(rawClaim({ height, bits: 0x01010000, locktime: height }), 'a time-domain coinbase height');
});

test('a claim cannot leave the vault below the 800-satoshi token dust', () => {
  const sizing = claimScenario();
  const exact = sizing.claim.fee + 800n + 800n;
  const { claim } = claimScenario({ value: exact });
  assert.equal(claim.transaction.outputs[0].valueSatoshis, 800n);
  accepts(claim, 'exactly 800 satoshis left');
  const below = clone(claim);
  below.transaction.outputs[0].valueSatoshis -= 1n;
  refuses(below, '799 satoshis left, the fee still within S to 3S');
  assert.throws(() => claimScenario({ value: exact - 1n }), /keep 800 satoshis/);
});

test('claim outputs pay scripts of at most 35 bytes, so K bounds the claim', () => {
  const p2sh32 = flattenBinArray([Uint8Array.of(0xaa, 0x20), new Uint8Array(32).fill(0x42), Uint8Array.of(0x87)]);
  assert.equal(p2sh32.length, 35);
  accepts(rawClaim({ payout: p2sh32 }), 'a P2SH32 payout');
  const long = new Uint8Array(36).fill(0x51);
  assert.equal(verifyNonstandard(rawClaim({ payout: p2sh32 })), true, 'control in the consensus-only VM');
  assert.notEqual(verifyNonstandard(rawClaim({ payout: long })), true, 'a 36-byte payout with its own leaf');
  assert.throws(() => claimScenario({ payout: long }), /at most 35 bytes/);
  assert.throws(() => claimScenario({ split: { ...SPLIT, donationLockingBytecode: long } }), /at most 35 bytes/);
});

// ---- fees ----

test('the vault pays 1 to 3 sat/byte of K plus the proof, and not a satoshi more or less', () => {
  for (const feeRate of [1, 2, 3]) accepts(claimScenario({ feeRate }).claim, `${feeRate} sat/byte`);
  const low = clone(claimScenario({ feeRate: 1 }).claim);
  low.transaction.outputs[0].valueSatoshis += 1n;
  refuses(low, 'one satoshi under the bound');
  const high = clone(claimScenario({ feeRate: 3 }).claim);
  high.transaction.outputs[0].valueSatoshis -= 1n;
  refuses(high, 'fee overpay: one satoshi over three times the bound');
  assert.throws(() => claimScenario({ feeRate: 4 }), /fee_rate/);
});

// ---- state and target ----

test('the next state is exactly the thread, the coinbase height, the claim target and the header nBits', () => {
  const { claim } = claimScenario();
  for (const [why, offset] of [['thread', 0], ['height', 4], ['target', 8], ['reference', 12]]) {
    const changed = clone(claim);
    changed.transaction.outputs[0].token.nft.commitment[offset] ^= 1;
    refuses(changed, why);
  }
});

test('the claim target eases by min(dh, 288) / threads, and the leaf commits to it', () => {
  const stateBits = 0x1f00ffff;
  for (const delta of [1, 4, 100, 288, 5000]) {
    const { claim } = claimScenario({ state: { height: CHIPNET_BLOCK.height - delta, targetBits: stateBits } });
    accepts(claim, `dh ${delta}`);
    const factor = BigInt(Math.min(delta, 288)) * 5000n / 4n + 5000n;
    assert.equal(claim.plan.targetBits, safa.encodeTokenTarget(safa.expandTokenTarget(stateBits) * factor / 10000n));
  }
  // A leaf that claims an easier target than the covenant computes.
  refuses(claimScenario({ leafTargetBits: 0x207fffff }).claim, 'an easier target in the leaf');
});

// ---- the market ----

function trade(claimLikeVault, { cashIn, tokensOut }) {
  const vault = structuredClone(claimLikeVault);
  const next = structuredClone(vault);
  next.valueSatoshis += cashIn;
  next.token.amount -= tokensOut;
  return {
    sourceOutputs: [vault, { lockingBytecode: Uint8Array.of(0x51), valueSatoshis: cashIn + 2000n }],
    transaction: {
      version: 2, locktime: 0,
      inputs: [
        { outpointTransactionHash: hash256(utf8('vault outpoint')), outpointIndex: 0, sequenceNumber: 0xffffffff, unlockingBytecode: safa.marketUnlockingBytecode(COVENANT) },
        { outpointTransactionHash: hash256(utf8('buyer')), outpointIndex: 0, sequenceNumber: 0xffffffff, unlockingBytecode: new Uint8Array() },
      ],
      outputs: [next, { lockingBytecode: p2pkh('77'), valueSatoshis: 1000n, token: { category: vault.token.category, amount: tokensOut } }],
    },
  };
}

test('the market trades at the virtual constant product and keeps the state, so proofs stay valid', () => {
  const scenario = claimScenario();
  const vault = scenario.claim.sourceOutputs[0];
  const num = vault.valueSatoshis + COVENANT.virtualSats;
  const cashIn = 1000000n;
  const fair = vault.token.amount - (num * vault.token.amount + num + cashIn - 1n) / (num + cashIn);
  accepts(trade(vault, { cashIn, tokensOut: fair }), 'a fair buy');
  refuses(trade(vault, { cashIn, tokensOut: fair + 1n }), 'one token too many');
  // The proof mined for the state still claims against the traded vault.
  const traded = trade(vault, { cashIn, tokensOut: fair }).transaction.outputs[0];
  const after = safa.buildClaim(COVENANT, {
    outpoint: { txid: hash256(utf8('traded vault')), vout: 0 },
    vault: { category: CATEGORY, value: traded.valueSatoshis, tokenAmount: traded.token.amount, commitment: traded.token.nft.commitment },
    payoutLockingBytecode: p2pkh('12'), proof: scenario.proof,
  });
  accepts(after);
});

// ---- limits and cost ----

test('the deepest proof and a full 10,000-byte unlocking bytecode verify', () => {
  accepts(claimScenario({ auxHeight: 16, coinbaseDepth: 25, split: SPLIT }).claim);
  const full = claimAtUnlockingSize(safa.MAX_UNLOCKING_BYTES);
  assert.equal(full.claim.transaction.inputs[0].unlockingBytecode.length, 10000);
  accepts(full.claim);
  assert.throws(() => claimAtUnlockingSize(safa.MAX_UNLOCKING_BYTES + 1), /10,000-byte/);
});

test('worst-case redeem size, operation cost and hash iterations stay within the VM limits', t => {
  const cases = {
    'h 0, d 0, no split': claimScenario({ otherTokens: 0, coinbaseDepth: 0 }),
    'h 1, d 1, split': claimScenario({ covenant: ONE_SAT, split: SPLIT }),
    'h 16, d 25, split': claimScenario({ auxHeight: 16, coinbaseDepth: 25, split: SPLIT }),
    'h 16, d 25, split, fallback': claimScenario({
      auxHeight: 16, coinbaseDepth: 25, split: SPLIT, bits: 0x1b0404cb,
      state: { height: CHIPNET_BLOCK.height - 4032, refBits: 0x1b0404cb },
    }),
    'h 16, d 25, split, 10,000-byte unlock': claimAtUnlockingSize(safa.MAX_UNLOCKING_BYTES),
  };
  let worstCost = 0;
  let worstHashing = 0;
  for (const [name, { claim }] of Object.entries(cases)) {
    accepts(claim, name);
    const metrics = inputMetrics(claim);
    assert.ok(metrics.operationCost <= metrics.maximumOperationCost, name);
    assert.ok(metrics.hashDigestIterations <= metrics.maximumHashDigestIterations, name);
    worstCost = Math.max(worstCost, metrics.operationCost / metrics.maximumOperationCost);
    worstHashing = Math.max(worstHashing, metrics.hashDigestIterations / metrics.maximumHashDigestIterations);
    t.diagnostic(`${name}: unlocking ${claim.transaction.inputs[0].unlockingBytecode.length} bytes, `
      + `op-cost ${metrics.operationCost} of ${metrics.maximumOperationCost}, `
      + `hash iterations ${metrics.hashDigestIterations} of ${metrics.maximumHashDigestIterations}, `
      + `claim ${claim.size} bytes within ${claim.plan.sizeBound}`);
  }
  t.diagnostic(`redeem script ${COVENANT.redeemScript.length} bytes (chipnet), K ${COVENANT.feeK}; `
    + `worst op-cost ${(100 * worstCost).toFixed(1)}% and hashing ${(100 * worstHashing).toFixed(1)}% of their limits`);
  assert.ok(COVENANT.redeemScript.length < 1500);
});

/** The deepest proof with the coinbase padded until the unlocking bytecode is `size` bytes. */
function claimAtUnlockingSize(size) {
  const build = padding => claimScenario({
    auxHeight: 16, coinbaseDepth: 25, split: SPLIT,
    payouts: [[312500000n, p2pkh('12')], [0n, new Uint8Array(padding).fill(0x51)]],
  });
  const first = build(6000);
  return build(6000 + size - first.claim.transaction.inputs[0].unlockingBytecode.length);
}

/**
 * A claim assembled without the JavaScript model's checks, to show the
 * covenant itself refuses what the model would refuse.
 */
function rawClaim({ stateHeight = CHIPNET_BLOCK.height - 10, height = CHIPNET_BLOCK.height, bits = CHIPNET_BLOCK.bits,
  locktime = height, payout = p2pkh('12') }) {
  const commitment = safa.encodeState({ thread: 1, height: stateHeight, targetBits: 0x2000ffff, refBits: CHIPNET_BLOCK.bits });
  const delta = Math.max(height - stateHeight, 1);
  const targetBits = safa.claimTargetBits(0x2000ffff, delta, COVENANT.threads);
  const leaf = {
    category: Uint8Array.from(CATEGORY).reverse(), lockHash: COVENANT.lockHash, stateHash: hash256(commitment),
    payoutHash: hash256(payout), targetBits, ext: new Uint8Array(32),
  };
  const output0 = safa.commitmentOutput(safa.leafHash(leaf), 0, 0);
  const coinbase = makeCoinbase({ height, output0, heightPush: encodeHeightPush(height) });
  const header = mineHeader({
    merkleRoot: hash256(coinbase.coinbase), time: CHIPNET_BLOCK.time, bits, target: safa.expandTokenTarget(targetBits),
  });
  const proof = safa.encodeProof({ header, height: 0, salt: 0, auxBranch: [], head: coinbase.head, tail: coinbase.tail, coinbaseBranch: [] });
  const value = 2000000n;
  const tokens = 420000000000n;
  const fee = BigInt(COVENANT.feeK + proof.length);
  const nextState = flattenBinArray([commitment.slice(0, 4), safa.uint32(height), safa.uint32(targetBits), header.slice(72, 76)]);
  const transaction = {
    version: 2, locktime,
    inputs: [{
      outpointTransactionHash: hash256(utf8('vault outpoint')), outpointIndex: 0, sequenceNumber: 0xfffffffe,
      unlockingBytecode: safa.claimUnlockingBytecode(COVENANT, { preimage: new Uint8Array(), payoutHash: leaf.payoutHash, proof }),
    }],
    outputs: [
      { lockingBytecode: COVENANT.lockingBytecode, valueSatoshis: value - fee - 800n, token: { category: CATEGORY, amount: tokens - 1000000n, nft: { capability: 'mutable', commitment: nextState } } },
      { lockingBytecode: payout, valueSatoshis: 800n, token: { category: CATEGORY, amount: 1000000n } },
    ],
  };
  assert.ok(encodeTransactionBch(transaction).length > 0);
  return {
    transaction,
    sourceOutputs: [{ lockingBytecode: COVENANT.lockingBytecode, valueSatoshis: value, token: { category: CATEGORY, amount: tokens, nft: { capability: 'mutable', commitment } } }],
  };
}
