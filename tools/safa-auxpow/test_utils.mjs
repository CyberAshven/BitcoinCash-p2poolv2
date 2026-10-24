import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import {
  bigIntToVmNumber, createVirtualMachineBch2026, encodeDataPush, flattenBinArray, hash256, hexToBin,
  numberToBinUint32LE, vmNumberToBigInt,
} from '@bitauth/libauth';
import * as safa from './lib.mjs';
import { chipnetPairs } from './rust_vectors.mjs';

export function temporaryInput(test, contents, name = 'input.json') {
  const directory = mkdtempSync(join(tmpdir(), 'safa-auxpow-'));
  const path = join(directory, name);
  writeFileSync(path, contents);
  test.after(() => rmSync(directory, { recursive: true, force: true }));
  return path;
}

export const p2pkh = byte => hexToBin(`76a914${byte.repeat(20)}88ac`);
/** The BIP34 height push BCHN writes: a minimal number push. */
export const encodeHeightPush = height => encodeDataPush(bigIntToVmNumber(BigInt(height)));
export const utf8 = text => new TextEncoder().encode(text);

/** Real chipnet block 329,001 from the Rust ASERT vectors: its time and nBits pass ASERT. */
const realChild = hexToBin(chipnetPairs.at(-1).child);
export const CHIPNET_BLOCK = Object.freeze({
  height: chipnetPairs.at(-1).parentHeight + 1,
  time: safa.readUint32(realChild, 68),
  bits: safa.readUint32(realChild, 72),
});

/** A tSAFA-like chipnet deployment: four threads, 420 tBCH of virtual AMM cash. */
export const COVENANT = safa.createCovenant({ network: 'chipnet', threads: 4, virtualSats: 42000000000n });
/** The token category, in display order as a txid is shown. */
export const CATEGORY = hash256(utf8('tSAFA test category'));

export function compactSize(value) {
  if (value < 253) return Uint8Array.of(value);
  if (value <= 0xffff) return Uint8Array.of(0xfd, value & 0xff, value >>> 8);
  return flattenBinArray([Uint8Array.of(0xfe), numberToBinUint32LE(value)]);
}

function uint64(value) {
  const bytes = new Uint8Array(8);
  new DataView(bytes.buffer).setBigUint64(0, BigInt(value), true);
  return bytes;
}

/**
 * A fixed-shape BCH coinbase: the BIP34 height and extranonce in its
 * scriptSig, output 0, then `payouts` ([value, locking bytecode]).
 */
export function makeCoinbase({
  height, output0, payouts = [[312500000n, p2pkh('12')]], extranonce = new Uint8Array(8).fill(7),
  sequence = 0xffffffff, locktime = 0, heightPush = encodeHeightPush(height),
}) {
  const scriptSig = flattenBinArray([heightPush, extranonce]);
  const head = flattenBinArray([
    numberToBinUint32LE(2), Uint8Array.of(1), new Uint8Array(32), hexToBin('ffffffff'),
    Uint8Array.of(scriptSig.length), scriptSig, numberToBinUint32LE(sequence), Uint8Array.of(1 + payouts.length),
  ]);
  const tail = flattenBinArray([
    ...payouts.map(([value, script]) => flattenBinArray([uint64(value), compactSize(script.length), script])),
    numberToBinUint32LE(locktime),
  ]);
  return { head, tail, coinbase: flattenBinArray([head, output0, tail]) };
}

/** An 80-byte header whose HASH256 meets `target`, searching the nonce. */
export function mineHeader({ merkleRoot, time, bits, target, version = 0x20000000, prev = new Uint8Array(32).fill(0x55) }) {
  for (let nonce = 0; nonce < 1 << 24; nonce += 1) {
    const header = flattenBinArray([
      numberToBinUint32LE(version), prev, merkleRoot, numberToBinUint32LE(time), numberToBinUint32LE(bits),
      numberToBinUint32LE(nonce),
    ]);
    if (safa.readUnsigned(hash256(header)) <= target) return header;
  }
  throw new Error('test mining exceeded its budget');
}

const DEFAULT_STATE = Object.freeze({
  thread: 1, height: CHIPNET_BLOCK.height - 10, targetBits: 0x2000ffff, refBits: CHIPNET_BLOCK.bits,
});

/**
 * A complete claim in the chipnet covenant, built from its parts: the vault
 * state, the leaf, a tree with `otherTokens` more leaves (or a synthetic
 * branch of `auxHeight` levels), the coinbase, `coinbaseDepth` siblings and
 * a header mined to the claim target. `leafTargetBits` commits (and mines)
 * a leaf with another target than the covenant computes.
 */
export function claimScenario({
  covenant = COVENANT, state = {}, value = 2000000n, tokenAmount = 420000000000n,
  category = CATEGORY, payout = p2pkh('12'), split = null, height = CHIPNET_BLOCK.height,
  time = CHIPNET_BLOCK.time, bits = CHIPNET_BLOCK.bits, otherTokens = 1, auxHeight,
  coinbaseDepth = 1, payouts, extranonce, feeRate = 1, locktime, outpoint, leafTargetBits,
} = {}) {
  const commitment = safa.encodeState({ ...DEFAULT_STATE, ...state });
  const vault = { category, value, tokenAmount, commitment };
  const leafInfo = safa.claimLeaf(covenant, { vault, payoutLockingBytecode: payout, split, height });
  if (leafTargetBits !== undefined) {
    leafInfo.leaf = { ...leafInfo.leaf, targetBits: leafTargetBits };
    leafInfo.leafHash = safa.leafHash(leafInfo.leaf);
    leafInfo.targetBits = leafTargetBits;
  }
  const wire = leafInfo.leaf.category;
  let tree;
  if (auxHeight === undefined) {
    const others = Array.from({ length: otherTokens }, (_, index) => ({
      category: new Uint8Array(32).fill(0x30 + index), hash: hash256(Uint8Array.of(0xa0, index)),
    }));
    const built = safa.buildTree([{ category: wire, hash: leafInfo.leafHash }, ...others]);
    tree = { root: built.root, height: built.height, salt: built.salt, branch: built.branches[0] };
  } else {
    const salt = 7;
    const leafSlot = safa.slot(wire, salt, auxHeight);
    const branch = Array.from({ length: auxHeight }, (_, level) => hash256(Uint8Array.of(0xaa, level)));
    tree = { root: safa.foldAux(leafInfo.leafHash, leafSlot, branch), height: auxHeight, salt, branch };
  }
  const output0 = safa.commitmentOutput(tree.root, tree.height, tree.salt);
  const coinbase = makeCoinbase({ height, output0, payouts, extranonce });
  const coinbaseBranch = Array.from({ length: coinbaseDepth }, (_, level) => hash256(Uint8Array.of(0xbb, level)));
  const merkleRoot = safa.coinbaseMerkleRoot(coinbase.coinbase, coinbaseBranch);
  const header = mineHeader({ merkleRoot, time, bits, target: safa.expandTokenTarget(leafInfo.targetBits) });
  const proof = safa.encodeProof({
    header, height: tree.height, salt: tree.salt, auxBranch: tree.branch, head: coinbase.head,
    tail: coinbase.tail, coinbaseBranch,
  });
  const claim = safa.buildClaim(covenant, {
    outpoint: outpoint ?? { txid: hash256(utf8('vault outpoint')), vout: 0 },
    vault, payoutLockingBytecode: payout, split, proof, feeRate, locktime,
  });
  return { covenant, vault, payout, split, leafInfo, tree, coinbase, header, proof, claim };
}

export function verify({ transaction, sourceOutputs }) {
  return createVirtualMachineBch2026(true).verify({ transaction, sourceOutputs });
}

/** The VM's metrics for input 0: operation cost, hash iterations and their limits. */
export function inputMetrics({ transaction, sourceOutputs }) {
  const state = createVirtualMachineBch2026(true).evaluate({ inputIndex: 0, sourceOutputs, transaction });
  if (state.error !== undefined) throw new Error(state.error);
  return state.metrics;
}

/** Replace a claim's unlocking arguments (split preimage, payout hash, proof). */
export function setUnlock(claim, covenant, { preimage, payoutHash, proof }) {
  claim.transaction.inputs[0].unlockingBytecode = flattenBinArray([
    encodeDataPush(preimage), encodeDataPush(payoutHash), encodeDataPush(proof), encodeDataPush(covenant.redeemScript),
  ]);
}

/**
 * Run `body` after the covenant's functions in a bare (non-standard) VM and
 * return the final stack as numbers or hex, or the error.
 */
export function runFunctions(network, body) {
  const bytecode = safa.compileLibrary(network, body);
  const transaction = {
    version: 2, locktime: 0,
    inputs: [{ outpointTransactionHash: new Uint8Array(32), outpointIndex: 0, sequenceNumber: 0, unlockingBytecode: new Uint8Array() }],
    outputs: [{ lockingBytecode: hexToBin('6a'.padEnd(60, '0')), valueSatoshis: 0n }],
  };
  const state = createVirtualMachineBch2026(false).evaluate({
    inputIndex: 0, transaction, sourceOutputs: [{ lockingBytecode: bytecode, valueSatoshis: 0n }],
  });
  return { error: state.error, stack: state.stack };
}

export const stackNumber = item => vmNumberToBigInt(item, { requireMinimalEncoding: true });

const OP_INVOKE = 0x8a;
const bareTransaction = {
  version: 2, locktime: 0,
  inputs: [{ outpointTransactionHash: new Uint8Array(32), outpointIndex: 0, sequenceNumber: 0, unlockingBytecode: new Uint8Array() }],
  outputs: [{ lockingBytecode: hexToBin('6a'.padEnd(60, '0')), valueSatoshis: 0n }],
};

/**
 * Invoke one covenant function many times cheaply: the library is compiled
 * once per network, and each call pushes its arguments (bytes, or numbers
 * as bigint) and invokes the function. Returns { error, stack }.
 */
export function functionCaller(network) {
  const library = safa.compileLibrary(network, '');
  const vm = createVirtualMachineBch2026(false);
  return (functionId, ...args) => {
    const body = flattenBinArray([
      ...args.map(arg => encodeDataPush(typeof arg === 'bigint' ? bigIntToVmNumber(arg) : arg)),
      encodeDataPush(bigIntToVmNumber(BigInt(functionId))), Uint8Array.of(OP_INVOKE),
    ]);
    const state = vm.evaluate({
      inputIndex: 0, transaction: bareTransaction,
      sourceOutputs: [{ lockingBytecode: flattenBinArray([library, body]), valueSatoshis: 0n }],
    });
    // A function may return several items; only the clean-stack rule objects.
    const cleanStack = /unexpected number of items on the stack/.test(state.error ?? '');
    return { error: cleanStack ? undefined : state.error, stack: state.stack };
  };
}
