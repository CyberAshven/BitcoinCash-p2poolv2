// Pickaxe's merge-mining v2 golden vectors (PR #70), read from P2Pool's Rust
// tests, checked against the covenant's own functions in the BCH 2026 VM.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { binToHex, flattenBinArray, hash256, hexToBin } from '@bitauth/libauth';
import * as safa from './lib.mjs';
import { auxpowVectors as v } from './rust_vectors.mjs';
import { functionCaller, makeCoinbase, p2pkh, runFunctions, stackNumber } from './test_utils.mjs';

const F = safa.FUNCTIONS;
const call = functionCaller('chipnet');
const ok = result => {
  assert.equal(result.error, undefined, result.error);
  return result.stack;
};
const fails = result => assert.notEqual(result.error, undefined, 'the covenant function accepted it');

const payout = p2pkh('12');
const ZERO = new Uint8Array(32);
const tokenA = {
  category: new Uint8Array(32).fill(0x11), lockHash: hash256(Uint8Array.of(0x51)), stateHash: new Uint8Array(32).fill(0xaa),
  payoutHash: hash256(payout), targetBits: 0x207fffff, ext: ZERO,
};
const tokenB = {
  category: new Uint8Array(32).fill(0x22), lockHash: hash256(Uint8Array.of(0x52)), stateHash: new Uint8Array(32).fill(0xbb),
  payoutHash: hash256(payout), targetBits: 0x1f00ffff, ext: ZERO,
};
const leafArgs = leaf => [leaf.category, leaf.lockHash, leaf.stateHash, leaf.payoutHash, safa.uint32(leaf.targetBits), leaf.ext];

test('leaf A and B preimages and hashes match the golden vectors', () => {
  assert.equal(binToHex(safa.leafPreimage(tokenA)), v.LEAF_A);
  assert.equal(binToHex(ok(call(F.leafHash, ...leafArgs(tokenA)))[0]), v.LEAF_A_HASH);
  assert.equal(binToHex(ok(call(F.leafHash, ...leafArgs(tokenB)))[0]), v.LEAF_B_HASH);
  assert.equal(binToHex(safa.leafHash(tokenB)), v.LEAF_B_HASH);
});

test('the two-token tree: slots 0 and 1 at h = 1, salt 0, root 5b61ecfe…1c38d0 and output 0', () => {
  const [a, b] = [hexToBin(v.LEAF_A_HASH), hexToBin(v.LEAF_B_HASH)];
  assert.equal(stackNumber(ok(call(F.slot, tokenA.category, safa.uint32(0), 1n))[0]), 0n);
  assert.equal(stackNumber(ok(call(F.slot, tokenB.category, safa.uint32(0), 1n))[0]), 1n);
  assert.equal(binToHex(ok(call(F.foldAux, a, 0n, b))[0]), v.TWO_TOKEN_ROOT);
  assert.equal(binToHex(ok(call(F.foldAux, b, 1n, a))[0]), v.TWO_TOKEN_ROOT);
  const output = ok(call(F.commitmentOutput, hexToBin(v.TWO_TOKEN_ROOT), 1n, safa.uint32(0)))[0];
  assert.equal(output.length, 53);
  assert.equal(binToHex(output), v.TWO_TOKEN_OUTPUT);
  const tree = safa.buildTree([{ category: tokenA.category, hash: a }, { category: tokenB.category, hash: b }]);
  assert.deepEqual([tree.height, tree.salt, tree.slots], [1, 0, [0, 1]]);
  assert.equal(binToHex(tree.root), v.TWO_TOKEN_ROOT);
});

test('the exact empty commitment', () => {
  assert.equal(
    binToHex(ok(call(F.commitmentOutput, ZERO, 0n, safa.uint32(0)))[0]),
    `00000000000000002c6a2a43544d4d02${'00'.repeat(32)}0000000000`,
  );
});

/** The covenant's proof verification, for a given leaf hash, category and token bits. */
function verifyProofBody(proof, leaf, category, bits) {
  return `<0x${binToHex(proof)}> <1> OP_INVOKE
    <0x${binToHex(leaf)}>
    <0x${binToHex(category)}> <6> OP_PICK <8> OP_PICK <9> OP_INVOKE
    <5> OP_ROLL <10> OP_INVOKE
    <5> OP_ROLL <5> OP_ROLL <11> OP_INVOKE
    <3> OP_ROLL OP_SWAP <3> OP_ROLL <3> OP_ROLL <12> OP_INVOKE
    OP_OVER <36> OP_SPLIT OP_NIP <32> OP_SPLIT OP_DROP OP_EQUALVERIFY
    <0x${binToHex(safa.uint32(bits))}> <3> OP_INVOKE <13> OP_INVOKE OP_1`;
}

test('the 224-byte one-leaf proof parses, rebuilds its txid and verifies for token A only', () => {
  const proof = hexToBin(v.PROOF);
  assert.equal(proof.length, 224);
  const [header, h, salt, aux, head, tail, branch] = ok(call(F.parseProof, proof));
  assert.deepEqual([header.length, stackNumber(h), salt.length, aux.length, head.length, tail.length], [80, 0n, 4, 0, 59, 38]);
  assert.equal(binToHex(branch), '44'.repeat(32));
  assert.equal(stackNumber(ok(call(F.coinbaseHeight, head))[0]), 325909n);
  const output0 = safa.commitmentOutput(hexToBin(v.LEAF_A_HASH), 0, 0);
  assert.equal(binToHex(ok(call(F.coinbaseMerkleRoot, head, output0, tail, new Uint8Array()))[0]), v.PROOF_COINBASE_TXID);
  assert.deepEqual(safa.decodeProof(proof).head, head);
  assert.deepEqual(safa.encodeProof(safa.decodeProof(proof)), proof);

  const verified = runFunctions('chipnet', verifyProofBody(proof, hexToBin(v.LEAF_A_HASH), tokenA.category, 0x207fffff));
  assert.equal(verified.error, undefined, verified.error);
  for (const stateHash of [new Uint8Array(32).fill(0xab), Uint8Array.from(tokenA.stateHash, (byte, i) => (i === 0 ? byte ^ 1 : byte))]) {
    const stale = safa.leafHash({ ...tokenA, stateHash });
    fails(runFunctions('chipnet', verifyProofBody(proof, stale, tokenA.category, 0x207fffff)));
  }
  // A harder token target than the header's work.
  fails(runFunctions('chipnet', verifyProofBody(proof, hexToBin(v.LEAF_A_HASH), tokenA.category, 0x03000001)));
  // The header's regtest nBits are above BCH's limit: both ASERT bands refuse them.
  for (const network of ['mainnet', 'chipnet']) {
    fails(functionCaller(network)(F.asertBand, header.slice(72, 76), header.slice(68, 72), 325909n));
  }
});

test('the proof decoder is canonical: trailing bytes, truncation, magic, h above 16, d above 25', () => {
  const proof = hexToBin(v.PROOF);
  fails(call(F.parseProof, flattenBinArray([proof, Uint8Array.of(0)])));
  fails(call(F.parseProof, proof.slice(0, -1)));
  for (const offset of [0, 4, 85]) {
    const changed = Uint8Array.from(proof);
    changed[offset] = offset === 85 ? 17 : changed[offset] ^ 1;
    fails(call(F.parseProof, changed));
  }
  const parts = safa.decodeProof(proof);
  const deep = Uint8Array.from(safa.encodeProof({ ...parts, coinbaseBranch: Array(25).fill(new Uint8Array(32).fill(0x44)) }));
  ok(call(F.parseProof, deep));
  deep[deep.length - 1 - 25 * 32] = 26;
  fails(call(F.parseProof, flattenBinArray([deep, new Uint8Array(32)])));
});

test('the whole split leaf: preimage, ext 79f31f39…, leaf c4027136…033e41', () => {
  const preimage = safa.splitPreimage({
    donationBps: 100, donationHash: hash256(p2pkh('d0')), feeBps: 100, feeHash: hash256(p2pkh('fe')),
  });
  assert.equal(binToHex(preimage), v.SPLIT_PREIMAGE);
  const [ext, dBps, dHash, fBps, fHash] = ok(call(F.parseSplit, preimage));
  assert.equal(binToHex(ext), v.SPLIT_EXT);
  assert.deepEqual([stackNumber(dBps), stackNumber(fBps)], [100n, 100n]);
  assert.deepEqual([dHash, fHash], [hash256(p2pkh('d0')), hash256(p2pkh('fe'))]);
  const leaf = { ...tokenA, ext };
  assert.equal(binToHex(safa.leafPreimage(leaf)), v.SPLIT_LEAF);
  assert.equal(binToHex(ok(call(F.leafHash, ...leafArgs(leaf)))[0]), v.SPLIT_LEAF_HASH);
  // No split: an empty push gives the zero ext.
  const none = ok(call(F.parseSplit, new Uint8Array()));
  assert.deepEqual(none[0], ZERO);
  assert.equal(stackNumber(none[1]) + stackNumber(none[3]), 0n);
});

test('a split preimage is canonical: rates, hashes, prefix, length, and never all-empty', () => {
  const good = hexToBin(v.SPLIT_PREIMAGE);
  const change = edit => {
    const bytes = Uint8Array.from(good);
    edit(bytes);
    return bytes;
  };
  ok(call(F.parseSplit, good));
  fails(call(F.parseSplit, change(bytes => bytes.set([0x11, 0x27], 1))));
  ok(call(F.parseSplit, change(bytes => bytes.set([0x10, 0x27], 1))));
  fails(call(F.parseSplit, change(bytes => bytes.fill(0, 3, 35))));
  fails(call(F.parseSplit, change(bytes => bytes.fill(0, 35, 37))));
  fails(call(F.parseSplit, change(bytes => bytes.fill(0, 1))));
  fails(call(F.parseSplit, change(bytes => { bytes[0] = 2; })));
  fails(call(F.parseSplit, good.slice(0, 68)));
  // A donation alone, or a fee alone, is a split.
  ok(call(F.parseSplit, change(bytes => bytes.fill(0, 35))));
  ok(call(F.parseSplit, change(bytes => bytes.fill(0, 1, 35))));
});

const shares = (reward, dBps, fBps, num, tokens) => {
  const [miner, donation, fee] = ok(call(F.splitShares, reward, BigInt(dBps), BigInt(fBps), num, tokens)).map(stackNumber);
  assert.deepEqual(safa.splitShares(reward, dBps, fBps, num, tokens), { miner, donation, fee });
  return [donation, fee, miner];
};

test('split shares round down in turn, as in the golden table', () => {
  const max = (1n << 63n) - 1n;
  // An AMM price of 1,000 satoshis per token keeps every share above 0.
  const rows = [
    [1n, 10000, 10000, [1n, 0n, 0n]], [1n, 9999, 9999, [0n, 0n, 1n]],
    [99n, 10000, 10000, [99n, 0n, 0n]], [99n, 9999, 9999, [98n, 0n, 1n]],
    [100n, 10000, 10000, [100n, 0n, 0n]], [100n, 9999, 9999, [99n, 0n, 1n]],
    [max, 10000, 10000, [max, 0n, 0n]],
    [max, 9999, 9999, [9222449699651090329n, 922244969965109n, 92233720369n]],
    [99n, 100, 200, [0n, 1n, 98n]], [1000000n, 10000, 500, [1000000n, 0n, 0n]],
    [1000000n, 100, 100, [10000n, 9900n, 980100n]], [1001n, 5000, 10000, [500n, 501n, 0n]],
    [77n, 0, 0, [0n, 0n, 77n]],
  ];
  for (const [reward, dBps, fBps, expected] of rows) {
    assert.deepEqual(shares(reward, dBps, fBps, 1000n, 1n), expected, `${reward} at ${dBps}/${fBps}`);
  }
});

test('a share worth under 1,000 satoshis at the AMM price stays with the miner', () => {
  // One satoshi per token: cash 600,000 plus virtual 400,000 for 1,000,000 tokens.
  const [num, tokens] = [1000000n, 1000000n];
  assert.equal(stackNumber(ok(call(F.keepShare, 999n, num, tokens))[0]), 0n);
  assert.equal(stackNumber(ok(call(F.keepShare, 1000n, num, tokens))[0]), 1000n);
  assert.deepEqual(shares(99900n, 100, 0, num, tokens), [0n, 0n, 99900n]);
  assert.deepEqual(shares(100000n, 100, 0, num, tokens), [1000n, 0n, 99000n]);
  // Dropping the donation leaves the fee as computed from the raw remainder.
  const raw = shares(99900n, 100, 200, 1000n, 1n);
  const claimed = shares(99900n, 100, 200, num, tokens);
  assert.deepEqual(claimed, [0n, raw[1], raw[2] + raw[0]]);
  // An AMM holding no tokens prices nothing: both shares go to the miner.
  assert.deepEqual(shares(1000000n, 100, 200, num, 0n), [0n, 0n, 1000000n]);
  // The whole split leaf's claim at one satoshi per token.
  assert.deepEqual(shares(1000000n, 100, 100, num, tokens), [10000n, 9900n, 980100n]);
  assert.deepEqual(shares(50000n, 100, 100, num, tokens), [0n, 0n, 50000n]);
});

test('token targets: canonical compact only, one shift, and encoding rounds down', () => {
  for (const bits of [0x207fffff, 0x03000001, 0x1d00ffff, 0x2001bffe, 0x04008000]) {
    const target = stackNumber(ok(call(F.decodeTokenTarget, safa.uint32(bits)))[0]);
    assert.equal(target, safa.expandTokenTarget(bits));
    assert.equal(binToHex(ok(call(F.encodeTokenTarget, target))[0]), binToHex(safa.uint32(bits)));
  }
  for (const invalid of [0x02000001, 0x21000001, 0x20800000, 0x1d000000]) {
    fails(call(F.decodeTokenTarget, safa.uint32(invalid)));
    assert.throws(() => safa.expandTokenTarget(invalid));
  }
  for (const target of [1n, 0x7fffffn, 0x800000n, 0x123456789n, safa.MAX_TOKEN_TARGET, (1n << 200n) + 12345n]) {
    const bits = ok(call(F.encodeTokenTarget, target))[0];
    assert.equal(binToHex(bits), binToHex(safa.uint32(safa.encodeTokenTarget(target))));
    assert.ok(safa.expandTokenTarget(safa.readUint32(bits)) <= target);
  }
});

test('header nBits decode as BCHN does, refusing zero, negative, overflowing and above the limit', () => {
  for (const bits of [0x1804dafe, 0x1d00ffff, 0x1a3106ee, 0x17030ecd, 0x01010000, 0x0300ffff]) {
    assert.equal(stackNumber(ok(call(F.decodeHeaderBits, safa.uint32(bits)))[0]), safa.decodeHeaderBits(bits));
  }
  for (const invalid of [0x00000000, 0x04800001, 0x23000001, 0x2100ffff, 0x1d010000, 0x207fffff, 0x04800000]) {
    fails(call(F.decodeHeaderBits, safa.uint32(invalid)));
    assert.equal(safa.decodeHeaderBits(invalid), null);
  }
});

test('the coinbase head: BIP34 heights as BCHN writes them, and the fixed shape', () => {
  const output0 = safa.commitmentOutput(ZERO, 0, 0);
  const head = push => makeCoinbase({ height: 1, output0, heightPush: hexToBin(push) }).head;
  assert.equal(stackNumber(ok(call(F.coinbaseHeight, head('0315f904')))[0]), 325909n);
  assert.equal(stackNumber(ok(call(F.coinbaseHeight, head('0111')))[0]), 17n);
  assert.equal(stackNumber(ok(call(F.coinbaseHeight, head('028000')))[0]), 128n);
  assert.equal(safa.coinbaseHeight(head('0315f904')), 325909);
  // Not minimal, negative, a small-integer opcode, or not a push.
  for (const push of ['021100', '0181', '51', '00']) fails(call(F.coinbaseHeight, head(push)));
  const good = head('0315f904');
  const broken = (offset, value) => {
    const bytes = Uint8Array.from(good);
    bytes[offset] = value;
    return bytes;
  };
  fails(call(F.coinbaseHeight, broken(4, 2)));
  fails(call(F.coinbaseHeight, broken(good.length - 1, 0)));
  fails(call(F.coinbaseHeight, broken(good.length - 1, 0xfd)));
  fails(call(F.coinbaseHeight, good.slice(0, -1)));
});
