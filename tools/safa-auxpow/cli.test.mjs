import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { binToHex, decodeTransactionBch, hash256, hexToBin } from '@bitauth/libauth';
import { readJson } from './cli.mjs';
import * as safa from './lib.mjs';
import { CATEGORY, CHIPNET_BLOCK, COVENANT, claimScenario, p2pkh, temporaryInput, utf8, verify } from './test_utils.mjs';

const cli = fileURLToPath(new URL('cli.mjs', import.meta.url));
const run = (...args) => spawnSync(process.execPath, [cli, ...args], { encoding: 'utf8' });
const deployment = { network: 'chipnet', threads: 4, virtual_sats: '42000000000', category: binToHex(CATEGORY) };
const SPLIT_JSON = { donation_bps: 100, donation_script: binToHex(p2pkh('d0')), fee_bps: 100, fee_script: binToHex(p2pkh('fe')) };
const SPLIT = {
  donationBps: 100, donationLockingBytecode: p2pkh('d0'), feeBps: 100, feeLockingBytecode: p2pkh('fe'),
};

function json(context, value, name) {
  return temporaryInput(context, JSON.stringify(value), name);
}

test('params prints the deployment covenant and its fee constant', context => {
  const result = run('params', json(context, deployment));
  assert.equal(result.status, 0, result.stderr);
  const output = JSON.parse(result.stdout);
  assert.equal(output.redeem_script, binToHex(COVENANT.redeemScript));
  assert.equal(output.fee_k, safa.CLAIM_FIXED_BYTES + 3 + COVENANT.redeemScript.length);
  assert.match(output.vault_address, /^bchtest:r/);
});

test('the chipnet tSAFA fixture reproduces byte for byte', context => {
  const fixture = JSON.parse(readFileSync(new URL('test_data/tsafa-chipnet-fixture.json', import.meta.url), 'utf8'));
  const result = run('params', json(context, fixture.deployment, 'fixture.json'));
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(JSON.parse(result.stdout), fixture.params);
  const covenant = safa.createCovenant({ network: 'chipnet', threads: 4, virtualSats: 84000000000n });
  assert.equal(fixture.params.redeem_script, binToHex(covenant.redeemScript));
  assert.equal(fixture.params.locking_bytecode, `aa20${binToHex(hash256(covenant.redeemScript))}87`);
  assert.equal(fixture.params.lock_hash, binToHex(hash256(hexToBin(fixture.params.locking_bytecode))));
  assert.equal(fixture.genesis.funding.txid, fixture.deployment.category);
});

test('the rebuilt Pickaxe tSAFA claim reproduces its verified transaction', context => {
  const fixture = JSON.parse(readFileSync(new URL('test_data/tsafa-chipnet-claim-result.json', import.meta.url), 'utf8'));
  const claimPath = fileURLToPath(new URL(`test_data/${fixture.claim_file}`, import.meta.url));
  const deploymentPath = json(context, fixture.deployment, 'deployment.json');
  const result = run('claim', deploymentPath, claimPath);
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(JSON.parse(result.stdout), fixture.result);
});

test('the original Pickaxe proof cannot claim the changed covenant lock hash', context => {
  const fixture = JSON.parse(readFileSync(new URL('test_data/tsafa-chipnet-claim-result.json', import.meta.url), 'utf8'));
  const rebuilt = JSON.parse(readFileSync(new URL(`test_data/${fixture.claim_file}`, import.meta.url), 'utf8'));
  const result = run('claim', json(context, fixture.deployment, 'deployment.json'),
    json(context, { ...rebuilt, proof: fixture.original_proof }, 'original-claim.json'));
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /covenant refuses this claim/);
  assert.equal(result.stdout, '');
});

test('leaf prints the token JSON p2poolv2_cli bch-auxpow reads, for one height', context => {
  const scenario = claimScenario({ split: SPLIT });
  const job = {
    vault_commitment: binToHex(scenario.vault.commitment), height: CHIPNET_BLOCK.height,
    payout_script: binToHex(p2pkh('12')), split: SPLIT_JSON,
  };
  const result = run('leaf', json(context, deployment, 'deployment.json'), json(context, job, 'job.json'));
  assert.equal(result.status, 0, result.stderr);
  const output = JSON.parse(result.stdout);
  assert.deepEqual(Object.keys(output.token), ['category', 'lock_hash', 'state_hash', 'payout_hash', 'target_bits', 'split']);
  assert.equal(output.token.category, binToHex(Uint8Array.from(CATEGORY).reverse()));
  assert.equal(output.token.target_bits, scenario.leafInfo.targetBits.toString(16).padStart(8, '0'));
  assert.equal(output.leaf_hash, binToHex(scenario.leafInfo.leafHash));
  assert.deepEqual(output.token.split, SPLIT_JSON);
});

test('claim builds, verifies and prints a complete claim transaction', context => {
  const scenario = claimScenario({ split: SPLIT });
  const input = {
    outpoint: { txid: binToHex(hash256(utf8('vault outpoint'))), vout: 0 },
    vault: {
      value_sats: scenario.vault.value.toString(), token_amount: scenario.vault.tokenAmount.toString(),
      commitment: binToHex(scenario.vault.commitment),
    },
    payout_script: binToHex(p2pkh('12')), split: SPLIT_JSON, proof: binToHex(scenario.proof), fee_rate: 2,
  };
  const deploymentPath = json(context, deployment, 'deployment.json');
  const result = run('claim', deploymentPath, json(context, input, 'claim.json'));
  assert.equal(result.status, 0, result.stderr);
  const output = JSON.parse(result.stdout);
  const transaction = decodeTransactionBch(hexToBin(output.hex));
  assert.notEqual(typeof transaction, 'string');
  assert.equal(verify({ transaction, sourceOutputs: scenario.claim.sourceOutputs }), true);
  assert.equal(output.fee_sats, String(2 * output.size_bound));
  assert.equal(output.reward, '1000000');
  // The covenant's refusal reaches the user as an error.
  const refused = run('claim', deploymentPath, json(context, { ...input, fee_rate: 4 }, 'refused.json'));
  assert.notEqual(refused.status, 0);
  assert.match(refused.stderr, /fee_rate/);
});

test('genesis prepares an unsigned chipnet genesis with one vault per thread and no minting baton', context => {
  const fundingScript = p2pkh('ab');
  const input = {
    network: 'chipnet', threads: 4, virtual_sats: '84000000000', supply_per_thread: '525000000000000',
    seed_sats: '2000000', start_height: CHIPNET_BLOCK.height, target_bits: '1d00ffff',
    ref_bits: CHIPNET_BLOCK.bits.toString(16).padStart(8, '0'),
    funding: { txid: binToHex(hash256(utf8('funding'))), vout: 0, value_sats: '8100000', script: binToHex(fundingScript) },
    change_script: binToHex(p2pkh('cd')), fee_rate: 1,
  };
  const result = run('genesis', json(context, input));
  assert.equal(result.status, 0, result.stderr);
  const output = JSON.parse(result.stdout);
  assert.equal(output.deployment.category, input.funding.txid);
  const transaction = decodeTransactionBch(hexToBin(output.unsigned_genesis));
  assert.notEqual(typeof transaction, 'string');
  assert.equal(transaction.outputs.length, 5);
  const covenant = safa.createCovenant({ network: 'chipnet', threads: 4, virtualSats: 84000000000n });
  transaction.outputs.slice(0, 4).forEach((vault, thread) => {
    assert.deepEqual(vault.lockingBytecode, covenant.lockingBytecode);
    assert.equal(vault.token.nft.capability, 'mutable');
    assert.deepEqual(safa.decodeState(vault.token.nft.commitment), {
      thread, height: CHIPNET_BLOCK.height, targetBits: 0x1d00ffff, refBits: CHIPNET_BLOCK.bits,
    });
    assert.equal(vault.token.amount, 525000000000000n);
  });
  assert.equal(transaction.outputs[4].token, undefined);
  assert.equal(transaction.outputs[4].valueSatoshis, 8100000n - 4n * 2000000n - BigInt(output.genesis_fee_sats));
  // The token rules accept the genesis (an anyone-can-spend stand-in for the signed input).
  assert.equal(verify({ transaction, sourceOutputs: [{ lockingBytecode: Uint8Array.of(0x51), valueSatoshis: 8100000n }] }), true);
  assert.match(output.sign_with, /^signrawtransactionwithwallet /);
  const mainnet = run('genesis', json(context, { ...input, network: 'mainnet' }, 'mainnet.json'));
  assert.notEqual(mainnet.status, 0);
  assert.match(mainnet.stderr, /chipnet only/);
  const notFirst = run('genesis', json(context, { ...input, funding: { ...input.funding, vout: 1 } }, 'vout.json'));
  assert.match(notFirst.stderr, /output 0/);
});

test('genesis accepts the 63-thread boundary and creates its last thread', context => {
  const input = {
    network: 'chipnet', threads: 63, virtual_sats: '84000000000', supply_per_thread: '420000000000',
    seed_sats: '10000', start_height: CHIPNET_BLOCK.height, target_bits: '1d00ffff',
    ref_bits: CHIPNET_BLOCK.bits.toString(16).padStart(8, '0'),
    funding: { txid: '11'.repeat(32), vout: 0, value_sats: '1000000', script: binToHex(p2pkh('ab')) },
    change_script: binToHex(p2pkh('cd')), fee_rate: 1,
  };
  const result = run('genesis', json(context, input));
  assert.equal(result.status, 0, result.stderr);
  const output = JSON.parse(result.stdout);
  assert.equal(output.deployment.threads, 63);
  assert.equal(output.vaults.length, 63);
  assert.equal(output.vaults[62].thread, 62);
  assert.equal(safa.decodeState(hexToBin(output.vaults[62].commitment)).thread, 62);
  const transaction = decodeTransactionBch(hexToBin(output.unsigned_genesis));
  assert.notEqual(typeof transaction, 'string');
  assert.equal(transaction.outputs.length, 64);
  assert.equal(verify({ transaction, sourceOutputs: [{ lockingBytecode: Uint8Array.of(0x51), valueSatoshis: 1000000n }] }), true);
});

test('genesis rejects 64 threads before reading funding or building a transaction', context => {
  const result = run('genesis', json(context, { network: 'chipnet', threads: 64, virtual_sats: '84000000000' }));
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /threads.*63/);
  assert.equal(result.stdout, '');
});

test('genesis rejects 288 threads before reading funding or building a transaction', context => {
  const result = run('genesis', json(context, { network: 'chipnet', threads: 288, virtual_sats: '84000000000' }));
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /threads.*63/);
  assert.equal(result.stdout, '');
});

test('malformed input fails the CLI with a reason', context => {
  const result = run('leaf', json(context, deployment, 'deployment.json'), json(context, { vault_commitment: 'gg' }, 'job.json'));
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /hexadecimal/);
  assert.match(run('unknown', 'file').stderr, /usage/);
});

test('the CLI JSON reader accepts exactly 1 MiB and rejects the next byte', context => {
  const exact = temporaryInput(context, '{}' + ' '.repeat(1024 * 1024 - 2));
  assert.deepEqual(readJson(exact), {});
  const excessive = temporaryInput(context, '{}' + ' '.repeat(1024 * 1024 - 1), 'large.json');
  assert.throws(() => readJson(excessive), /exceeds 1 MiB/);
});
