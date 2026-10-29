// Command line for the SAFA AuxPoW covenant. Public data only: it never
// reads keys, never signs, and never broadcasts.
//
//   node cli.mjs params  deployment.json            the covenant of a deployment
//   node cli.mjs leaf    deployment.json job.json   a miner's leaf for one height
//   node cli.mjs claim   deployment.json claim.json a complete claim transaction
//   node cli.mjs genesis genesis.json               a chipnet deployment's genesis
import { closeSync, fstatSync, openSync, readSync } from 'node:fs';
import { pathToFileURL } from 'node:url';
import { binToHex, encodeTransactionBch, hash256 } from '@bitauth/libauth';
import * as safa from './lib.mjs';

const USAGE = 'usage: node cli.mjs params deployment.json | leaf deployment.json job.json'
  + ' | claim deployment.json claim.json | genesis genesis.json';

export function readJson(path) {
  const handle = openSync(path, 'r');
  try {
    if (!fstatSync(handle).isFile()) throw new Error('input must be a regular file');
    const buffer = Buffer.alloc(1024 * 1024 + 1);
    let length = 0;
    let received;
    do {
      received = readSync(handle, buffer, length, buffer.length - length, null);
      length += received;
    } while (received !== 0 && length < buffer.length);
    if (length > 1024 * 1024) throw new Error('input JSON exceeds 1 MiB');
    return JSON.parse(buffer.subarray(0, length).toString('utf8'));
  } finally {
    closeSync(handle);
  }
}

function amount(value, name) {
  if (typeof value === 'number' ? !Number.isSafeInteger(value) : !/^\d+$/.test(value ?? '')) {
    throw new Error(`${name} must be a whole number of base units, as a number or a decimal string`);
  }
  return BigInt(value);
}

function bits(value, name) {
  if (typeof value !== 'string' || !/^[0-9a-fA-F]{8}$/.test(value)) {
    throw new Error(`${name} must be eight hexadecimal digits, such as "1d00ffff"`);
  }
  return Number.parseInt(value, 16);
}

function integer(value, name) {
  if (!Number.isSafeInteger(value) || value < 0) throw new Error(`${name} must be a non-negative integer`);
  return value;
}

/** The deployment descriptor: network, threads, virtual satoshis, and the category once known. */
function covenantOf(deployment) {
  return safa.createCovenant({
    network: deployment.network, threads: deployment.threads,
    virtualSats: amount(deployment.virtual_sats, 'virtual_sats'),
  });
}

function categoryOf(deployment) {
  return safa.fromHex(deployment.category, 32, 'category');
}

function splitOf(split) {
  if (split === undefined || split === null) return null;
  return {
    donationBps: integer(split.donation_bps, 'donation_bps'),
    donationLockingBytecode: safa.fromHex(split.donation_script ?? '', undefined, 'donation_script'),
    feeBps: integer(split.fee_bps, 'fee_bps'),
    feeLockingBytecode: safa.fromHex(split.fee_script ?? '', undefined, 'fee_script'),
  };
}

function hexBits(value) {
  return value.toString(16).padStart(8, '0');
}

function params(deployment) {
  const covenant = covenantOf(deployment);
  return {
    network: covenant.network, threads: covenant.threads, virtual_sats: covenant.virtualSats.toString(),
    redeem_script: binToHex(covenant.redeemScript), redeem_script_bytes: covenant.redeemScript.length,
    fee_k: covenant.feeK, locking_bytecode: binToHex(covenant.lockingBytecode),
    lock_hash: binToHex(covenant.lockHash), vault_address: safa.vaultAddress(covenant),
  };
}

/**
 * The leaf a miner commits for one vault at one height. `token` is the JSON
 * `p2poolv2_cli bch-auxpow build` and `verify` read; hashes are written as
 * hashed (the category in wire order, as the VM reads it).
 */
function leaf(deployment, job) {
  const covenant = covenantOf(deployment);
  const split = splitOf(job.split);
  const info = safa.claimLeaf(covenant, {
    vault: { category: categoryOf(deployment), commitment: safa.fromHex(job.vault_commitment, 16, 'vault_commitment') },
    payoutLockingBytecode: safa.fromHex(job.payout_script, undefined, 'payout_script'),
    split, height: integer(job.height, 'height'),
  });
  const token = {
    category: binToHex(info.leaf.category), lock_hash: binToHex(info.leaf.lockHash),
    state_hash: binToHex(info.leaf.stateHash), payout_hash: binToHex(info.leaf.payoutHash),
    target_bits: hexBits(info.targetBits),
  };
  if (split !== null) {
    token.split = {
      donation_bps: split.donationBps, donation_script: binToHex(split.donationLockingBytecode),
      fee_bps: split.feeBps, fee_script: binToHex(split.feeLockingBytecode),
    };
  }
  return {
    token, leaf_hash: binToHex(info.leafHash), ext: binToHex(info.leaf.ext),
    split_preimage: binToHex(info.preimage), height_delta: info.heightDelta,
  };
}

/** A complete claim, verified in the BCH 2026 VM before it is printed. */
function claim(deployment, input) {
  const covenant = covenantOf(deployment);
  const category = categoryOf(deployment);
  const vault = {
    category, value: amount(input.vault?.value_sats, 'vault.value_sats'),
    tokenAmount: amount(input.vault?.token_amount, 'vault.token_amount'),
    commitment: safa.fromHex(input.vault?.commitment, 16, 'vault.commitment'),
  };
  const outpoint = {
    txid: safa.fromHex(input.outpoint?.txid, 32, 'outpoint.txid'), vout: integer(input.outpoint?.vout, 'outpoint.vout'),
  };
  const built = safa.buildClaim(covenant, {
    outpoint, vault, payoutLockingBytecode: safa.fromHex(input.payout_script, undefined, 'payout_script'),
    split: splitOf(input.split), proof: safa.fromHex(input.proof, undefined, 'proof'),
    feeRate: input.fee_rate ?? 1, locktime: input.locktime === undefined ? undefined : integer(input.locktime, 'locktime'),
  });
  const verified = safa.verifyTransaction(built);
  if (verified !== true) throw new Error(`the covenant refuses this claim: ${verified}`);
  const encoded = encodeTransactionBch(built.transaction);
  return {
    txid: binToHex(hash256(encoded).reverse()), hex: binToHex(encoded), size: encoded.length,
    size_bound: built.plan.sizeBound, fee_sats: built.fee.toString(), coinbase_height: built.plan.height,
    target_bits: hexBits(built.plan.targetBits), reward: built.plan.reward.toString(),
    shares: Object.fromEntries(Object.entries(built.plan.shares).map(([key, value]) => [key, value.toString()])),
    next_state: binToHex(built.plan.nextState),
  };
}

/**
 * A deployment's genesis, unsigned, with 1 to 63 threads. Chipnet only:
 * mainnet waits for an independent covenant review and the owner's approval.
 */
function genesis(input) {
  if (input.network !== 'chipnet') throw new Error('genesis is prepared for chipnet only');
  if (!Number.isSafeInteger(input.threads) || input.threads < 1 || input.threads > safa.MAX_THREADS) {
    throw new Error(`threads must be an integer from 1 to ${safa.MAX_THREADS}`);
  }
  const deployment = { network: input.network, threads: input.threads, virtual_sats: input.virtual_sats };
  const covenant = covenantOf(deployment);
  const funding = {
    txid: safa.fromHex(input.funding?.txid, 32, 'funding.txid'), vout: integer(input.funding?.vout, 'funding.vout'),
    valueSatoshis: amount(input.funding?.value_sats, 'funding.value_sats'),
    lockingBytecode: safa.fromHex(input.funding?.script, undefined, 'funding.script'),
  };
  const built = safa.buildGenesis(covenant, {
    funding, changeLockingBytecode: safa.fromHex(input.change_script, undefined, 'change_script'),
    supplyPerThread: amount(input.supply_per_thread, 'supply_per_thread'),
    seedSats: amount(input.seed_sats, 'seed_sats'), startHeight: integer(input.start_height, 'start_height'),
    targetBits: bits(input.target_bits, 'target_bits'), refBits: bits(input.ref_bits, 'ref_bits'),
    feeRate: input.fee_rate ?? 1,
  });
  const unsigned = binToHex(encodeTransactionBch(built.transaction));
  const prevtx = [{
    txid: input.funding.txid, vout: 0, scriptPubKey: input.funding.script,
    amount: Number(funding.valueSatoshis) / 1e8,
  }];
  return {
    deployment: { ...deployment, virtual_sats: covenant.virtualSats.toString(), category: input.funding.txid },
    vault_address: safa.vaultAddress(covenant), redeem_script_bytes: covenant.redeemScript.length, fee_k: covenant.feeK,
    vaults: built.vaults.map((vault, thread) => ({
      thread, value_sats: vault.valueSatoshis.toString(), token_amount: vault.token.amount.toString(),
      commitment: binToHex(vault.token.nft.commitment),
    })),
    genesis_fee_sats: built.fee.toString(), genesis_size_estimate: built.size,
    unsigned_genesis: unsigned,
    sign_with: `signrawtransactionwithwallet ${unsigned} '${JSON.stringify(prevtx)}'`,
    note: 'Not broadcast. The owner signs the funding input with their own wallet and decides whether to send it.',
  };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    const [command, first, second, extra] = process.argv.slice(2);
    let result;
    if (extra !== undefined || first === undefined) throw new Error(USAGE);
    if (command === 'params' && second === undefined) result = params(readJson(first));
    else if (command === 'leaf' && second !== undefined) result = leaf(readJson(first), readJson(second));
    else if (command === 'claim' && second !== undefined) result = claim(readJson(first), readJson(second));
    else if (command === 'genesis' && second === undefined) result = genesis(readJson(first));
    else throw new Error(USAGE);
    console.log(JSON.stringify(result, null, 2));
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
