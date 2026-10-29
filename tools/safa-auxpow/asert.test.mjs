// The covenant's aserti3-2d arithmetic and header band, against BCHN's
// specification runs and real BCH headers (the vectors P2Pool's Rust side
// checks), and the DAA-change fallback band.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { hexToBin } from '@bitauth/libauth';
import * as safa from './lib.mjs';
import { ASERT_RUNS, asertRun, chipnetPairs, mainnetPairs } from './rust_vectors.mjs';
import { functionCaller, stackNumber } from './test_utils.mjs';

const F = safa.FUNCTIONS;
const callers = { mainnet: functionCaller('mainnet'), chipnet: functionCaller('chipnet') };
const SPEC_HALF_LIFE = 172800n;

function band(network, bits, time, height) {
  const result = callers[network](F.asertBand, safa.uint32(bits), safa.uint32(time), BigInt(height));
  assert.equal(result.error, undefined, result.error);
  const ok = stackNumber(result.stack[0]) === 1n;
  assert.equal(ok, safa.asertBandOk(network, bits, time, height), `JavaScript model at ${bits.toString(16)}`);
  return ok;
}

test('all 14,000 aserti3-2d specification vectors match in the VM', () => {
  let checked = 0;
  for (const name of ASERT_RUNS) {
    const run = asertRun(name);
    const reference = safa.setCompact(run.anchorBits);
    assert.equal(run.lines.length, run.iterations, name);
    for (const line of run.lines) {
      const result = callers.mainnet(
        F.asertTarget, reference, line.time - run.anchorParentTime, line.height - run.anchorHeight, SPEC_HALF_LIFE,
      );
      assert.equal(result.error, undefined, `${name} ${line.height}: ${result.error}`);
      const target = stackNumber(result.stack[0]);
      assert.equal(safa.getCompact(target), line.bits, `${name} at height ${line.height}`);
      assert.equal(target, safa.calculateAsert(
        reference, line.time - run.anchorParentTime, line.height - run.anchorHeight, SPEC_HALF_LIFE,
      ));
      checked += 1;
    }
  }
  assert.equal(checked, 14000);
});

function header(hex) {
  const bytes = hexToBin(hex);
  return { time: safa.readUint32(bytes, 68), bits: safa.readUint32(bytes, 72) };
}

test('real mainnet and chipnet headers pass the band at their coinbase height', () => {
  for (const [network, pairs] of [['mainnet', mainnetPairs], ['chipnet', chipnetPairs]]) {
    assert.equal(pairs.length, 9);
    for (const { parentHeight, parent, child } of pairs) {
      const { time, bits } = header(child);
      assert.equal(band(network, bits, time, parentHeight + 1), true, `${network} block ${parentHeight + 1}`);
      // With the parent's real time, ASERT gives the child's bits exactly
      // (bar chipnet's minimum-difficulty block).
      const expected = safa.getCompact(safa.asertTarget(network, parentHeight + 1, header(parent).time));
      assert.ok(expected === bits || (network === 'chipnet' && bits === safa.POW_LIMIT_BITS));
    }
  }
});

test('mainnet: computed / 2 to computed x 1.05, exact at the edges, and no powLimit allowance', () => {
  const { anchorHeight, anchorParentTime } = safa.NETWORKS.mainnet;
  const at = bits => band('mainnet', bits, anchorParentTime + 600, anchorHeight + 1);
  assert.equal(at(0x1804dafe), true);
  assert.equal(at(0x18026d7f), true);
  assert.equal(at(0x18026d7e), false);
  assert.equal(at(0x18051924), true);
  assert.equal(at(0x18051925), false);
  assert.equal(at(safa.POW_LIMIT_BITS), false);
});

test('chipnet: computed / 8 to computed x 8, or nBits exactly powLimit', () => {
  const { anchorHeight, anchorParentTime } = safa.NETWORKS.chipnet;
  const height = anchorHeight + 1 + 120;
  const time = anchorParentTime + 600;
  const computed = safa.asertTarget('chipnet', height, time);
  const at = target => band('chipnet', safa.getCompact(target), time, height);
  assert.equal(at(computed), true);
  assert.equal(at(computed / 8n), true);
  assert.equal(at(computed / 16n), false);
  assert.equal(at(computed * 2n), true);
  assert.equal(at(computed * 8n), true);
  assert.equal(at(computed * 16n), false);
  assert.equal(band('chipnet', safa.POW_LIMIT_BITS, time, height), true);
});

test('chipnet block 329001 nBits pass with its header time two hours before its real parent', () => {
  const pair = chipnetPairs.at(-1);
  const parent = header(pair.parent);
  const child = header(pair.child);
  const time = parent.time - 7200;
  assert.equal(pair.parentHeight + 1, 329001);
  assert.equal(child.bits, 0x1a3106ee);
  assert.equal(parent.time - time, 7200);
  // Only the timestamp is moved: this tests the ASERT band, not the
  // modified header's proof of work. The real parent's ASERT target gives
  // the child's real nBits, while its own time can be two hours earlier.
  assert.equal(safa.getCompact(safa.asertTarget('chipnet', 329001, parent.time)), child.bits);
  assert.equal(band('chipnet', child.bits, time, 329001), true);
});

test('chipnet accepts the exact computed x 8 upper bound', () => {
  const { anchorHeight, anchorParentTime } = safa.NETWORKS.chipnet;
  const height = anchorHeight + 10001;
  const time = anchorParentTime + 600;
  // Clamp ASERT to 1 so both the bound and bound + 1 are exactly
  // representable as nBits; compact precision must not round +1 away.
  assert.equal(safa.asertTarget('chipnet', height, time), 1n);
  assert.equal(safa.setCompact(0x01080000), 8n);
  assert.equal(band('chipnet', 0x01080000, time, height), true);
});

test('chipnet rejects the exact computed x 8 upper bound plus one', () => {
  const { anchorHeight, anchorParentTime } = safa.NETWORKS.chipnet;
  const height = anchorHeight + 10001;
  const time = anchorParentTime + 600;
  assert.equal(safa.asertTarget('chipnet', height, time), 1n);
  assert.equal(safa.setCompact(0x01090000), 9n);
  assert.equal(band('chipnet', 0x01090000, time, height), false);
});

test('a Bitcoin header fails the band on both networks', () => {
  // BTC mainnet nBits near BCH's height 900,000 era: about 300 times harder.
  for (const btcBits of [0x17030ecd, 0x1702349e]) {
    const mainnet = mainnetPairs.at(-1);
    const chipnet = chipnetPairs.at(-1);
    assert.equal(band('mainnet', btcBits, header(mainnet.child).time, mainnet.parentHeight + 1), false);
    assert.equal(band('chipnet', btcBits, header(chipnet.child).time, chipnet.parentHeight + 1), false);
  }
});

test('invalid nBits and heights at or below the anchor are refused outright', () => {
  const { anchorHeight, anchorParentTime, anchorBits } = safa.NETWORKS.mainnet;
  for (const invalid of [0x00000000, 0x04800001, 0x23000001, 0x2100ffff, 0x1d010000]) {
    const result = callers.mainnet(F.asertBand, safa.uint32(invalid), safa.uint32(anchorParentTime), BigInt(anchorHeight + 1));
    assert.notEqual(result.error, undefined, invalid.toString(16));
  }
  for (const height of [0n, BigInt(anchorHeight)]) {
    const result = callers.mainnet(F.asertBand, safa.uint32(anchorBits), safa.uint32(anchorParentTime), height);
    assert.notEqual(result.error, undefined);
  }
});

test('the band agrees with the JavaScript model across times and nBits', () => {
  const { child, parentHeight } = mainnetPairs.at(-1);
  const real = header(child);
  for (const offset of [-86400 * 30, -3600, 0, 7200, 86400 * 30, 86400 * 400]) {
    for (const bits of [real.bits, 0x18010000, 0x1803ffff, 0x18100000, 0x1c00ffff, 0x17030ecd]) {
      band('mainnet', bits, real.time + offset, parentHeight + 1);
    }
  }
});

test('the fallback band: ref >> 4 up to ref x 1.1, only after 4,032 blocks', () => {
  const call = callers.chipnet;
  const reference = 0x1804dafe;
  const referenceTarget = safa.setCompact(reference);
  const at = (target, dh) => {
    const bits = safa.getCompact(target);
    const result = call(F.fallbackBand, safa.uint32(bits), safa.uint32(reference));
    assert.equal(result.error, undefined, result.error);
    const ok = stackNumber(result.stack[0]) === 1n;
    assert.equal(ok, safa.fallbackBandOk(bits, reference, dh));
    return ok;
  };
  assert.equal(at(referenceTarget, 4032), true);
  assert.equal(at(referenceTarget >> 4n, 4032), true);
  assert.equal(at((referenceTarget >> 4n) - (1n << 160n), 4032), false);
  assert.equal(at(referenceTarget * 11n / 10n, 4032), true);
  assert.equal(at(referenceTarget * 12n / 10n, 4032), false);
  // min(dh / 288, 4) is 4 wherever the band applies, so the shift is a constant.
  for (const dh of [4032, 4320, 100000]) assert.equal(Math.min(Math.floor(dh / 288), 4), 4);
  assert.equal(safa.fallbackBandOk(safa.getCompact(referenceTarget >> 4n), reference, 4031), false);
});
