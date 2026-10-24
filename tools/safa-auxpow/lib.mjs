// The SAFA AuxPoW covenant (merge-mining common format v2): compiling it per
// deployment, and a JavaScript model of every rule it enforces, used to build
// claims and genesis transactions and to check the covenant in the tests.
// The covenant in the BCH 2026 VM is the authority; this model only mirrors it.
import { readFileSync } from 'node:fs';
import {
  bigIntToVmNumber,
  binToHex,
  createCompilerBch,
  createVirtualMachineBch2026,
  encodeDataPush,
  encodeTransactionBch,
  flattenBinArray,
  hash256,
  hexToBin,
  lockingBytecodeToCashAddress,
  numberToBinUint32LE,
  sha256,
} from '@bitauth/libauth';

/** BCH's proof-of-work limit on mainnet and the test networks, 2^224 - 1. */
export const POW_LIMIT = (1n << 224n) - 1n;
/** `GetCompact` of {@link POW_LIMIT}. */
export const POW_LIMIT_BITS = 0x1d00ffff;
/** The easiest canonical token target, `0x7fffff << 232` (bits `0x207fffff`). */
export const MAX_TOKEN_TARGET = 0x7fffffn << 232n;
/** A claim pays `floor(vault tokens / 420,000)`. */
export const REWARD_DIVISOR = 420000n;
/** Satoshis on the miner's, the donation's and the fee's claim outputs. */
export const OUTPUT_SATS = 800n;
/** A split share worth less than this at the AMM price stays with the miner. */
export const MIN_SHARE_SATS = 1000n;
/** The DAA eases by at most this many blocks per claim. */
export const EASE_CAP = 288;
/** Blocks without a claim after which the fallback nBits band applies. */
export const FALLBACK_DELTA = 4032;
/** Claim outputs pay scripts of at most 35 bytes (P2PKH, P2SH20, P2SH32), as K assumes. */
export const MAX_PAYOUT_SCRIPT_BYTES = 35;
export const MAX_TREE_HEIGHT = 16;
export const MAX_COINBASE_BRANCH = 25;
export const MAX_SPLIT_BPS = 10000;
/** BCH's standard unlocking bytecode limit (2025 VM limits). */
export const MAX_UNLOCKING_BYTES = 10000;
/** Fee rates the covenant allows, in satoshis per byte of its size bound. */
export const MIN_FEE_RATE = 1;
export const MAX_FEE_RATE = 3;
/** Locktimes at or above this are times, not heights. */
export const LOCKTIME_THRESHOLD = 500000000;

/**
 * A claim's bytes besides the proof and the redeem script, at their largest:
 * version 4, input count 1, outpoint 36, unlocking length 3, sequence 4,
 * output count 1, the vault output 104 (8 value, 1 length, 60 token prefix
 * with a 16-byte commitment and a 9-byte amount, 35 P2SH32), three token
 * outputs of 87 (8, 1, 43 token prefix, a script of at most 35 bytes:
 * P2PKH, P2SH20 or P2SH32), locktime 4, the split push 70, the payout hash
 * push 33, the proof's push prefix 3, and the 53-byte commitment output that
 * the proof leaves out (agreed with Pickaxe). The redeem script and its push
 * prefix are added per deployment: K = 577 + 3 + redeem length.
 */
export const CLAIM_FIXED_BYTES = 4 + 1 + 36 + 3 + 4 + 1 + 104 + 3 * 87 + 4 + 70 + 33 + 3 + 53;

/**
 * Per-network ASERT parameters, from BCHN's chain parameters
 * (`src/chainparams.cpp`: `asertAnchorParams`, `nASERTHalfLife`,
 * `fPowAllowMinDifficultyBlocks`), and P2Pool's agreed band below the
 * computed target. Regtest and scalenet have no anchor, so no deployment.
 */
export const NETWORKS = Object.freeze({
  mainnet: Object.freeze({
    anchorHeight: 661647, anchorBits: 0x1804dafe, anchorParentTime: 1605447844,
    halfLife: 172800, allowPowLimit: false, hardDivisor: 2, cashAddressPrefix: 'bitcoincash',
  }),
  chipnet: Object.freeze({
    anchorHeight: 16844, anchorBits: 0x1d00ffff, anchorParentTime: 1605451779,
    halfLife: 3600, allowPowLimit: true, hardDivisor: 8, cashAddressPrefix: 'bchtest',
  }),
});

/** Function identifiers in safa.cash. */
export const FUNCTIONS = Object.freeze({
  parseProof: 1, coinbaseHeight: 2, decodeTokenTarget: 3, encodeTokenTarget: 4,
  decodeHeaderBits: 5, asertTarget: 6, asertBand: 7, fallbackBand: 8, slot: 9,
  foldAux: 10, commitmentOutput: 11, coinbaseMerkleRoot: 12, checkWork: 13,
  leafHash: 14, parseSplit: 15, checkSplitSlot: 16, splitShares: 17, keepShare: 18,
  checkTokenOutput: 19, placeOutput: 20,
});

const SOURCE = readFileSync(new URL('safa.cash', import.meta.url), 'utf8');
const MAIN_MARKER = '// == main ==';
const [LIBRARY_SOURCE, MAIN_SOURCE] = SOURCE.split(MAIN_MARKER);
if (MAIN_SOURCE === undefined) throw new Error('safa.cash has no main marker');
const compilerVm = createVirtualMachineBch2026(false);
const ZERO32 = new Uint8Array(32);

function substitute(text, values) {
  return text.replace(/\{\{([A-Z_]+)\}\}/g, (_, name) => {
    if (!(name in values)) throw new Error(`no value for {{${name}}}`);
    return values[name].toString();
  });
}

/** Compile CashASM to bytecode. */
export function compileCashAsm(text) {
  const compiler = createCompilerBch({ scripts: { lock: text }, vm: compilerVm });
  const result = compiler.generateBytecode({ scriptId: 'lock', data: {} });
  if (!result.success) throw new Error(JSON.stringify(result.errors));
  return result.bytecode;
}

export function networkParameters(network) {
  const parameters = NETWORKS[network];
  if (parameters === undefined) throw new Error(`unknown network ${network}: mainnet or chipnet`);
  return parameters;
}

function libraryValues(network) {
  const parameters = networkParameters(network);
  return {
    POW_LIMIT, ALLOW_POW_LIMIT: parameters.allowPowLimit ? 1 : 0,
    ANCHOR_HEIGHT: parameters.anchorHeight, ANCHOR_PARENT_TIME: parameters.anchorParentTime,
    ANCHOR_TARGET: setCompact(parameters.anchorBits), HALF_LIFE: parameters.halfLife,
    HARD_DIVISOR: parameters.hardDivisor,
  };
}

/** The covenant's functions for `network`, followed by `body` (CashASM). */
export function compileLibrary(network, body) {
  return compileCashAsm(`${substitute(LIBRARY_SOURCE, libraryValues(network))}\n${body}`);
}

function pushPrefixLength(length) {
  if (length <= 75) return 1;
  if (length <= 255) return 2;
  if (length <= 65535) return 3;
  return 5;
}

function positiveInteger(value, name, maximum = Number.MAX_SAFE_INTEGER) {
  if (!Number.isSafeInteger(value) || value < 1 || value > maximum) {
    throw new Error(`${name} must be an integer from 1 to ${maximum}`);
  }
  return value;
}

/**
 * Compile one deployment: `network` (mainnet or chipnet), the number of
 * `threads` and the AMM's `virtualSats`. The fee constant K includes the
 * redeem script, so it is found by compiling until it stops changing.
 */
export function createCovenant({ network, threads, virtualSats }) {
  networkParameters(network);
  positiveInteger(threads, 'threads', 0x7fffffff);
  const virtual = BigInt(virtualSats);
  if (virtual < 0n || virtual > (1n << 62n)) throw new Error('virtual_sats must be from 0 to 2^62');
  const library = substitute(LIBRARY_SOURCE, libraryValues(network));
  let feeK = CLAIM_FIXED_BYTES + 3 + 2000;
  for (let round = 0; round < 8; round += 1) {
    const main = substitute(MAIN_SOURCE, {
      FEE_K: feeK, THREADS: threads, VIRTUAL_SATS: virtual, MAX_TOKEN_TARGET,
    });
    const redeemScript = compileCashAsm(`${library}\n${main}`);
    const next = CLAIM_FIXED_BYTES + pushPrefixLength(redeemScript.length) + redeemScript.length;
    if (next === feeK) {
      const lockingBytecode = flattenBinArray([
        Uint8Array.of(0xaa, 0x20), hash256(redeemScript), Uint8Array.of(0x87),
      ]);
      return Object.freeze({
        network, threads, virtualSats: virtual, feeK, redeemScript, lockingBytecode,
        lockHash: hash256(lockingBytecode),
      });
    }
    feeK = next;
  }
  throw new Error('the fee constant did not converge');
}

/** The vault's token-aware P2SH32 CashAddress. */
export function vaultAddress(covenant) {
  const result = lockingBytecodeToCashAddress({
    bytecode: covenant.lockingBytecode,
    prefix: networkParameters(covenant.network).cashAddressPrefix,
    tokenSupport: true,
  });
  if (typeof result === 'string') throw new Error(result);
  return result.address;
}

// ---- bytes and numbers ----

export function fromHex(value, length, name = 'hexadecimal field') {
  if (typeof value !== 'string' || !/^(?:[0-9a-fA-F]{2})*$/.test(value)) {
    throw new Error(`${name} must be hexadecimal bytes`);
  }
  const decoded = hexToBin(value);
  if (length !== undefined && decoded.length !== length) throw new Error(`${name} must be ${length} bytes`);
  return decoded;
}

export function uint32(value, name = 'value') {
  if (!Number.isInteger(value) || value < 0 || value > 0xffffffff) {
    throw new Error(`${name} must be an unsigned 32-bit integer`);
  }
  return numberToBinUint32LE(value);
}

function uint16(value) {
  if (!Number.isInteger(value) || value < 0 || value > 0xffff) throw new Error('value must be an unsigned 16-bit integer');
  return Uint8Array.of(value & 0xff, value >>> 8);
}

export function readUint32(bytes, offset = 0) {
  return new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength).getUint32(offset, true);
}

function readUint16(bytes, offset) {
  return new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength).getUint16(offset, true);
}

/** An unsigned little-endian number. */
export function readUnsigned(bytes) {
  let value = 0n;
  for (let index = bytes.length - 1; index >= 0; index -= 1) value = (value << 8n) | BigInt(bytes[index]);
  return value;
}

export function sha256Hash(bytes) {
  return sha256.hash(bytes);
}

function equalBytes(left, right) {
  return left.length === right.length && left.every((byte, index) => byte === right[index]);
}

// ---- targets ----

/** A canonical token target: 3 <= exp <= 32 and 0 < mantissa < 0x800000. */
export function expandTokenTarget(bits) {
  uint32(bits, 'target bits');
  const exponent = bits >>> 24;
  const mantissa = bits & 0xffffff;
  if (exponent < 3 || exponent > 32 || mantissa === 0 || mantissa >= 0x800000) {
    throw new Error('target bits are not a canonical token target');
  }
  return BigInt(mantissa) << BigInt(8 * (exponent - 3));
}

/** Canonical token bits for a target, rounded down as the covenant does. */
export function encodeTokenTarget(target) {
  if (typeof target !== 'bigint' || target < 1n || target > MAX_TOKEN_TARGET) {
    throw new Error('token target must be from 1 to 0x7fffff << 232');
  }
  const size = Math.max(bigIntToVmNumber(target).length, 3);
  const mantissa = Number(target >> BigInt(8 * (size - 3)));
  return (size * 0x1000000 + mantissa) >>> 0;
}

/** BCHN's `SetCompact`: null for a negative, zero or overflowing value. */
export function setCompact(bits) {
  uint32(bits, 'bits');
  const size = bits >>> 24;
  const word = bits & 0x7fffff;
  let value;
  if (size <= 3) {
    value = BigInt(word >>> (8 * (3 - size)));
  } else {
    if (word !== 0 && (size > 34 || (word > 0xff && size > 33) || (word > 0xffff && size > 32))) return null;
    value = BigInt(word) << BigInt(8 * (size - 3));
  }
  if ((word !== 0 && (bits & 0x800000) !== 0) || value === 0n) return null;
  return value;
}

/** BCHN's `GetCompact` for a non-negative value. */
export function getCompact(value) {
  let size = value === 0n ? 0 : Math.ceil(value.toString(2).length / 8);
  let compact = size <= 3
    ? Number(value << BigInt(8 * (3 - size)))
    : Number(value >> BigInt(8 * (size - 3)));
  if ((compact & 0x800000) !== 0) {
    compact >>>= 8;
    size += 1;
  }
  return (compact | (size << 24)) >>> 0;
}

/** A header's nBits as the covenant reads them: null if invalid or above powLimit. */
export function decodeHeaderBits(bits) {
  const value = setCompact(bits);
  return value === null || value > POW_LIMIT ? null : value;
}

/** BCHN's `CalculateASERT` with the proof-of-work limit of every BCH network. */
export function calculateAsert(reference, timeDiff, heightDiff, halfLife) {
  const span = BigInt(timeDiff) - 600n * BigInt(heightDiff);
  const exponent = ((span - 600n) * 65536n) / BigInt(halfLife);
  const shifts = exponent >> 16n;
  const fraction = exponent - shifts * 65536n;
  const factor = 65536n + ((195766423245049n * fraction + 971821376n * fraction * fraction
    + 5127n * fraction * fraction * fraction + (1n << 47n)) >> 48n);
  let next = reference * factor;
  const shift = shifts - 16n;
  next = shift <= 0n ? next >> (-shift < 256n ? -shift : 256n) : next << (shift < 256n ? shift : 256n);
  if (next === 0n) next = 1n;
  return next > POW_LIMIT ? POW_LIMIT : next;
}

/** The target ASERT gives a block at `height` whose parent has time `time`. */
export function asertTarget(network, height, time) {
  const parameters = networkParameters(network);
  const heightDiff = height - 1 - parameters.anchorHeight;
  if (heightDiff < 0) return null;
  return calculateAsert(
    setCompact(parameters.anchorBits), time - parameters.anchorParentTime, heightDiff, parameters.halfLife,
  );
}

/** The covenant's ASERT band for a header's nBits, time and coinbase height. */
export function asertBandOk(network, bits, time, height) {
  const parameters = networkParameters(network);
  const target = decodeHeaderBits(bits);
  if (target === null) return false;
  if (parameters.allowPowLimit && bits === POW_LIMIT_BITS) return true;
  const computed = asertTarget(network, height, time);
  if (computed === null) return false;
  return target >= computed / BigInt(parameters.hardDivisor) && target * 100n <= computed * 105n;
}

/**
 * The DAA-change fallback band, from 4,032 blocks without a claim:
 * ref >> 4 <= target <= ref * 1.1 (min(dh / 288, 4) is always 4 there).
 */
export function fallbackBandOk(bits, refBits, heightDelta) {
  const target = decodeHeaderBits(bits);
  const reference = decodeHeaderBits(refBits);
  if (target === null || reference === null || heightDelta < FALLBACK_DELTA) return false;
  return target >= reference >> 4n && target * 10n <= reference * 11n;
}

/** The claim's target bits: the state's target eased by min(dh, 288) / threads. */
export function claimTargetBits(stateBits, heightDelta, threads) {
  const factor = BigInt(Math.min(heightDelta, EASE_CAP)) * 5000n / BigInt(threads) + 5000n;
  let target = expandTokenTarget(stateBits) * factor / 10000n;
  if (target < 1n) target = 1n;
  if (target > MAX_TOKEN_TARGET) target = MAX_TOKEN_TARGET;
  return encodeTokenTarget(target);
}

// ---- thread state ----

export function encodeState({ thread, height, targetBits, refBits }) {
  if (height >= LOCKTIME_THRESHOLD) throw new Error('state height must be a block height');
  expandTokenTarget(targetBits);
  return flattenBinArray([
    uint32(thread, 'thread'), uint32(height, 'state height'), uint32(targetBits, 'target bits'),
    uint32(refBits, 'reference bits'),
  ]);
}

export function decodeState(bytes) {
  if (!(bytes instanceof Uint8Array) || bytes.length !== 16) throw new Error('a thread state is 16 bytes');
  return {
    thread: readUint32(bytes, 0), height: readUint32(bytes, 4),
    targetBits: readUint32(bytes, 8), refBits: readUint32(bytes, 12),
  };
}

// ---- merge-mining common format v2 ----

/** `u32le(SHA256(02 || salt || category)[0..4]) mod 2^h`, category as hashed. */
export function slot(category, salt, height) {
  const digest = sha256Hash(flattenBinArray([Uint8Array.of(2), uint32(salt, 'salt'), category]));
  return readUint32(digest) % 2 ** height;
}

export function nodeHash(left, right) {
  return sha256Hash(flattenBinArray([Uint8Array.of(1), left, right]));
}

export function leafPreimage({ category, lockHash, stateHash, payoutHash, targetBits, ext }) {
  for (const [value, name] of [[category, 'category'], [lockHash, 'lock_hash'], [stateHash, 'state_hash'],
    [payoutHash, 'payout_hash'], [ext, 'ext']]) {
    if (!(value instanceof Uint8Array) || value.length !== 32) throw new Error(`${name} must be 32 bytes`);
  }
  return flattenBinArray([
    hexToBin('43544d4c02'), category, lockHash, stateHash, payoutHash, uint32(targetBits), ext,
  ]);
}

export function leafHash(fields) {
  return sha256Hash(flattenBinArray([Uint8Array.of(0), leafPreimage(fields)]));
}

export function foldAux(leaf, leafSlot, branch) {
  let current = leaf;
  let index = leafSlot;
  for (const sibling of branch) {
    current = index % 2 === 1 ? nodeHash(sibling, current) : nodeHash(current, sibling);
    index = Math.floor(index / 2);
  }
  return current;
}

/** The 53-byte coinbase output 0. */
export function commitmentOutput(root, height, salt) {
  return flattenBinArray([
    hexToBin('00000000000000002c6a2a43544d4d02'), root, Uint8Array.of(height), uint32(salt, 'salt'),
  ]);
}

/**
 * The tree for `leaves` ({ category, hash }), as P2Pool's and Pickaxe's
 * builders make it: the smallest height that holds them and the first salt
 * that gives each category its own slot.
 */
export function buildTree(leaves) {
  if (leaves.length === 0) return { root: ZERO32, height: 0, salt: 0, branches: [] };
  if (leaves.length > 16) throw new Error('at most 16 tokens per coinbase');
  const minimum = Math.ceil(Math.log2(leaves.length));
  for (let height = minimum; height <= MAX_TREE_HEIGHT; height += 1) {
    for (let salt = 0; salt < 65536; salt += 1) {
      const slots = leaves.map(leaf => slot(leaf.category, salt, height));
      if (new Set(slots).size !== slots.length) continue;
      let level = Array.from({ length: 2 ** height }, () => ZERO32);
      leaves.forEach((leaf, index) => { level[slots[index]] = leaf.hash; });
      const levels = [level];
      while (level.length > 1) {
        const parents = [];
        for (let index = 0; index < level.length; index += 2) parents.push(nodeHash(level[index], level[index + 1]));
        level = parents;
        levels.push(level);
      }
      const branches = slots.map(leafSlot => {
        let index = leafSlot;
        return levels.slice(0, -1).map(nodes => {
          const sibling = nodes[index ^ 1];
          index >>= 1;
          return sibling;
        });
      });
      return { root: level[0], height, salt, slots, branches };
    }
  }
  throw new Error('no salt gives every token its own slot');
}

// ---- split ----

/** The 69-byte split preimage of { donationBps, donationHash, feeBps, feeHash }. */
export function splitPreimage({ donationBps, donationHash = ZERO32, feeBps, feeHash = ZERO32 }) {
  const bytes = flattenBinArray([
    Uint8Array.of(3), uint16(donationBps), donationHash, uint16(feeBps), feeHash,
  ]);
  return parseSplit(bytes) && bytes;
}

/** Read a revealed preimage as the covenant does; null for no split (empty). */
export function parseSplit(bytes) {
  if (bytes.length === 0) return null;
  if (bytes.length !== 69 || bytes[0] !== 3) throw new Error('a split preimage is 69 bytes starting with 03');
  const read = offset => {
    const bps = readUint16(bytes, offset);
    const hash = bytes.slice(offset + 2, offset + 34);
    if (bps > MAX_SPLIT_BPS || (bps === 0) !== equalBytes(hash, ZERO32)) throw new Error('a split slot is not canonical');
    return { bps, hash };
  };
  const donation = read(1);
  const fee = read(35);
  if (donation.bps === 0 && fee.bps === 0) throw new Error('an empty split is the zero ext, not a preimage');
  return { donationBps: donation.bps, donationHash: donation.hash, feeBps: fee.bps, feeHash: fee.hash };
}

export function splitExt(preimage) {
  return preimage.length === 0 ? ZERO32 : sha256Hash(preimage);
}

/**
 * The shares of `reward`: the donation, then the fee from what it leaves; a
 * share worth under 1,000 satoshis at the AMM price, `share * num / tokens`
 * with `num` the vault's cash plus its virtual satoshis, goes to the miner.
 */
export function splitShares(reward, donationBps, feeBps, num, tokens) {
  const donation = reward * BigInt(donationBps) / 10000n;
  const fee = (reward - donation) * BigInt(feeBps) / 10000n;
  const keep = share => (tokens > 0n && share * num / tokens >= MIN_SHARE_SATS ? share : 0n);
  const paidDonation = keep(donation);
  const paidFee = keep(fee);
  return { miner: reward - paidDonation - paidFee, donation: paidDonation, fee: paidFee };
}

// ---- AuxProof v2 ----

export function encodeProof({ header, height, salt, auxBranch, head, tail, coinbaseBranch }) {
  if (header.length !== 80 || height > MAX_TREE_HEIGHT || auxBranch.length !== height
    || coinbaseBranch.length > MAX_COINBASE_BRANCH || head.length > 65535 || tail.length > 65535) {
    throw new Error('proof parts exceed the v2 limits');
  }
  return flattenBinArray([
    hexToBin('43544d5002'), header, Uint8Array.of(height), uint32(salt, 'salt'), ...auxBranch,
    uint16(head.length), head, uint16(tail.length), tail, Uint8Array.of(coinbaseBranch.length), ...coinbaseBranch,
  ]);
}

export function decodeProof(bytes) {
  let offset = 0;
  const take = count => {
    if (offset + count > bytes.length) throw new Error('not a canonical AuxProof v2');
    const part = bytes.slice(offset, offset + count);
    offset += count;
    return part;
  };
  if (!equalBytes(take(5), hexToBin('43544d5002'))) throw new Error('not a canonical AuxProof v2');
  const header = take(80);
  const height = take(1)[0];
  if (height > MAX_TREE_HEIGHT) throw new Error('not a canonical AuxProof v2');
  const salt = readUint32(take(4));
  const auxBranch = Array.from({ length: height }, () => take(32));
  const head = take(readUint16(take(2), 0));
  const tail = take(readUint16(take(2), 0));
  const depth = take(1)[0];
  if (depth > MAX_COINBASE_BRANCH) throw new Error('not a canonical AuxProof v2');
  const coinbaseBranch = Array.from({ length: depth }, () => take(32));
  if (offset !== bytes.length) throw new Error('not a canonical AuxProof v2');
  return { header, height, salt, auxBranch, head, tail, coinbaseBranch };
}

/** The BIP34 height of a fixed-shape coinbase head, as the covenant reads it. */
export function coinbaseHeight(head) {
  const length = head[41];
  if (head.length !== 47 + length || length > 100
    || !equalBytes(head.slice(4, 41), hexToBin(`01${'00'.repeat(32)}ffffffff`))
    || head[head.length - 1] < 1 || head[head.length - 1] > 0xfc) {
    throw new Error('the coinbase head is malformed');
  }
  const push = head[42];
  if (push < 1 || push > 5 || length < 1 + push) throw new Error('the coinbase has no BIP34 height');
  const bytes = head.slice(43, 43 + push);
  const last = bytes[bytes.length - 1];
  if ((last & 0x80) !== 0 || (last === 0 && (bytes.length === 1 || (bytes[bytes.length - 2] & 0x80) === 0))) {
    throw new Error('the BIP34 height is not a minimal positive number');
  }
  return Number(readUnsigned(bytes));
}

export function coinbaseMerkleRoot(coinbase, branch) {
  return branch.reduce((current, sibling) => hash256(flattenBinArray([current, sibling])), hash256(coinbase));
}

// ---- claims ----

function vaultSourceOutput(covenant, vault) {
  return {
    lockingBytecode: covenant.lockingBytecode,
    valueSatoshis: BigInt(vault.value),
    token: {
      category: vault.category, amount: BigInt(vault.tokenAmount),
      nft: { capability: 'mutable', commitment: vault.commitment },
    },
  };
}

/**
 * The leaf a miner commits for a vault in its current state, paying
 * `payoutLockingBytecode` with `split`, in a coinbase at `height`: the
 * target eases with the blocks since the state, so the leaf depends on the
 * height being mined. `vault` is { category (display order), commitment };
 * `split` is null or { donationBps, donationLockingBytecode, feeBps,
 * feeLockingBytecode }.
 */
export function claimLeaf(covenant, { vault, payoutLockingBytecode, split = null, height }) {
  const state = decodeState(vault.commitment);
  if (!Number.isInteger(height) || height >= LOCKTIME_THRESHOLD) throw new Error('the height must be a block height');
  if (state.height >= height) throw new Error('the coinbase is not newer than the thread state');
  for (const script of [payoutLockingBytecode, split?.donationLockingBytecode, split?.feeLockingBytecode]) {
    if (script !== undefined && script.length > MAX_PAYOUT_SCRIPT_BYTES) {
      throw new Error('claim outputs pay scripts of at most 35 bytes: P2PKH, P2SH20 or P2SH32');
    }
  }
  const heightDelta = height - state.height;
  const targetBits = claimTargetBits(state.targetBits, heightDelta, covenant.threads);
  const preimage = split === null ? new Uint8Array() : splitPreimage({
    donationBps: split.donationBps,
    donationHash: split.donationBps === 0 ? ZERO32 : hash256(split.donationLockingBytecode),
    feeBps: split.feeBps,
    feeHash: split.feeBps === 0 ? ZERO32 : hash256(split.feeLockingBytecode),
  });
  const payoutHash = hash256(payoutLockingBytecode);
  const leaf = {
    category: Uint8Array.from(vault.category).reverse(), lockHash: covenant.lockHash,
    stateHash: hash256(vault.commitment), payoutHash, targetBits, ext: splitExt(preimage),
  };
  return { state, heightDelta, targetBits, preimage, payoutHash, leaf, leafHash: leafHash(leaf) };
}

/**
 * Everything a claim of `proof` against `vault` must be: its target, leaf,
 * reward, shares, outputs after the vault, next state and fee bound.
 * `vault` is { category (display order), value, tokenAmount, commitment }.
 */
export function planClaim(covenant, { vault, payoutLockingBytecode, split = null, proof }) {
  const parts = decodeProof(proof);
  const height = coinbaseHeight(parts.head);
  const { state, heightDelta, targetBits, preimage, payoutHash, leaf } = claimLeaf(
    covenant, { vault, payoutLockingBytecode, split, height },
  );
  const tokens = BigInt(vault.tokenAmount);
  const reward = tokens / REWARD_DIVISOR;
  if (reward <= 0n) throw new Error('the vault has too few tokens for a reward');
  const shares = split === null
    ? { miner: reward, donation: 0n, fee: 0n }
    : splitShares(reward, split.donationBps, split.feeBps, BigInt(vault.value) + covenant.virtualSats, tokens);
  const outputs = [
    [shares.miner, payoutLockingBytecode],
    [shares.donation, split?.donationLockingBytecode],
    [shares.fee, split?.feeLockingBytecode],
  ].filter(([amount]) => amount > 0n).map(([amount, lockingBytecode]) => ({
    lockingBytecode, valueSatoshis: OUTPUT_SATS, token: { category: vault.category, amount },
  }));
  const nextState = encodeState({
    thread: state.thread, height, targetBits, refBits: readUint32(parts.header, 72),
  });
  const sizeBound = covenant.feeK + proof.length;
  return {
    proof, parts, height, heightDelta, targetBits, leaf, leafHash: leafHash(leaf), preimage, payoutHash,
    reward, shares, outputs, nextState, sizeBound,
  };
}

/** A market spend unlocks with the redeem script alone. */
export function marketUnlockingBytecode(covenant) {
  return encodeDataPush(covenant.redeemScript);
}

export function claimUnlockingBytecode(covenant, { preimage, payoutHash, proof }) {
  const unlocking = flattenBinArray([
    encodeDataPush(preimage), encodeDataPush(payoutHash), encodeDataPush(proof),
    encodeDataPush(covenant.redeemScript),
  ]);
  if (unlocking.length > MAX_UNLOCKING_BYTES) throw new Error('the claim exceeds the 10,000-byte unlocking limit');
  return unlocking;
}

/**
 * The claim transaction: the vault (`outpoint` { txid in display order,
 * vout }) is its only input; it pays `feeRate` (1 to 3) satoshis per byte of
 * the covenant's size bound. Unsigned and complete: the covenant needs no
 * signature.
 */
export function buildClaim(covenant, { outpoint, vault, payoutLockingBytecode, split = null, proof,
  feeRate = 1, locktime }) {
  if (!Number.isInteger(feeRate) || feeRate < MIN_FEE_RATE || feeRate > MAX_FEE_RATE) {
    throw new Error('fee_rate must be 1, 2 or 3 satoshis per byte');
  }
  const plan = planClaim(covenant, { vault, payoutLockingBytecode, split, proof });
  const fee = BigInt(feeRate * plan.sizeBound);
  const vaultValue = BigInt(vault.value) - fee - OUTPUT_SATS * BigInt(plan.outputs.length);
  if (vaultValue < OUTPUT_SATS) throw new Error('the vault cannot pay the claim fee and outputs and keep 800 satoshis');
  const lockTime = locktime ?? plan.height;
  if (lockTime < plan.height || lockTime >= LOCKTIME_THRESHOLD) {
    throw new Error('the locktime must be a height at or above the coinbase height');
  }
  const sourceOutputs = [vaultSourceOutput(covenant, vault)];
  const transaction = {
    version: 2, locktime: lockTime,
    inputs: [{
      outpointTransactionHash: outpoint.txid, outpointIndex: outpoint.vout, sequenceNumber: 0xfffffffe,
      unlockingBytecode: claimUnlockingBytecode(covenant, plan),
    }],
    outputs: [{
      lockingBytecode: covenant.lockingBytecode, valueSatoshis: vaultValue,
      token: {
        category: vault.category, amount: BigInt(vault.tokenAmount) - plan.reward,
        nft: { capability: 'mutable', commitment: plan.nextState },
      },
    }, ...plan.outputs],
  };
  const size = encodeTransactionBch(transaction).length;
  return { plan, fee, size, transaction, sourceOutputs };
}

export function verifyTransaction({ transaction, sourceOutputs }) {
  return createVirtualMachineBch2026(true).verify({ transaction, sourceOutputs });
}

// ---- deployment ----

/**
 * A deployment's genesis: `threads` vaults, each with `supplyPerThread`
 * tokens, `seedSats` satoshis and a mutable NFT in its initial state, and
 * change. `funding` must be output 0 of its transaction: its txid becomes
 * the token category (CashTokens genesis). No minting baton is created, so
 * no further threads can ever exist.
 */
export function buildGenesis(covenant, { funding, changeLockingBytecode, supplyPerThread, seedSats,
  startHeight, targetBits, refBits, feeRate = 1 }) {
  if (funding.vout !== 0) throw new Error('the funding output must be output 0 of its transaction');
  if (decodeHeaderBits(refBits) === null) throw new Error('ref_bits must be a valid nBits at most powLimit');
  const supply = BigInt(supplyPerThread);
  const seed = BigInt(seedSats);
  if (supply < REWARD_DIVISOR || supply * BigInt(covenant.threads) > (1n << 63n) - 1n) {
    throw new Error('supply_per_thread must give a reward and fit the 2^63 - 1 token limit');
  }
  if (seed < 10000n) throw new Error('seed_sats must be at least 10,000 satoshis per thread');
  const vaults = Array.from({ length: covenant.threads }, (_, thread) => ({
    lockingBytecode: covenant.lockingBytecode, valueSatoshis: seed,
    token: {
      category: funding.txid, amount: supply,
      nft: { capability: 'mutable', commitment: encodeState({ thread, height: startHeight, targetBits, refBits }) },
    },
  }));
  const change = { lockingBytecode: changeLockingBytecode, valueSatoshis: 0n };
  const transaction = {
    version: 2, locktime: 0,
    inputs: [{
      outpointTransactionHash: funding.txid, outpointIndex: 0, sequenceNumber: 0xffffffff,
      unlockingBytecode: new Uint8Array(),
    }],
    outputs: [...vaults, change],
  };
  // A P2PKH input's signature (at most 73 bytes with ECDSA) and key, with their pushes, add 108 bytes.
  const size = encodeTransactionBch(transaction).length + 108;
  const fee = BigInt(size * feeRate);
  change.valueSatoshis = BigInt(funding.valueSatoshis) - seed * BigInt(covenant.threads) - fee;
  if (change.valueSatoshis < 546n) throw new Error('the funding output cannot pay the vaults, fee and change');
  return { transaction, fee, size, vaults };
}


export { binToHex, hash256, hexToBin };
