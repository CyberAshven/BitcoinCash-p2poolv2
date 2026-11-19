# BCH P2Pool Job Declaration

A P2Pool node on Bitcoin Cash serves Stratum V2 Job Declaration: a miner
declares its own block template and is still paid by P2Pool's direct PPLNS
outputs. The declared coinbase must carry those exact outputs, so a generic
client that expects one custodial pool payout needs P2Pool support: either
[`POST /jd_prepare`](#read-only-preparation-for-an-external-declarator),
which Pickaxe uses, or the
[`0x5052` extension](#payout-outputs-on-the-job-declaration-connection).
Protocol references: [SV2 Mining Protocol](https://stratumprotocol.org/specification/05-mining-protocol/)
and [SV2 Job Declaration Protocol](https://stratumprotocol.org/specification/06-job-declaration-protocol/).

## Running it

| Key | Default | Effect |
|---|---|---|
| `[stratum] sv2_port` | unset | Opens the SV2 listener, for Mining and Job Declaration connections |
| `sv2_authority_public_key`, `sv2_authority_secret_key`, `sv2_cert_validity_secs` | unset, unset, 3600 | Noise encryption; clients pin the public key ([setup](bch-quickstart.md#stratum-v2)) |
| `sv2_coinbase_only_from` | `["127.0.0.0/8", "::1/128"]` | Clients that may use Coinbase-Only ([below](#which-clients-may-use-coinbase-only)) |
| `merge_mining_tokens` | unset | Tokens the node merge-mines ([Auxiliary work proofs](bch-auxpow.md#merge-mining-tokens-on-a-public-node)) |
| `[[submit_nodes]]` | none | Extra BCHN nodes a found block also goes to |

## Roles and modes

- A **Mining connection** opens an extended channel that negotiates
  `REQUIRES_WORK_SELECTION` (bit 1) and installs declared jobs with
  `SetCustomMiningJob`.
- A separate **Job Declaration connection** allocates job tokens. In
  **Full-Template** mode (`DECLARE_TX_DATA`, bit 0) it declares the
  template and reveals its transactions; in **Coinbase-Only** mode (bit
  clear) it reveals none. Setup success flags are zero in both modes.

## Job tokens

`AllocateMiningJobToken.user_identifier` is a mining identity with the
options of a mining channel's ([Miner options](miner-options.md#stratum-v2)):
`donation=` chooses the token's donation, or the node's own without it
([Donation](miner-options.md#donation)), and `min-payout=` or
`fee-tolerance=` its payout level. The channel that installs the job must
have chosen the same donation (`donation-mismatch`). Allocation has no
error reply, so a bad option closes the connection, with the reason in the
node's log.

The allocation's outputs are the required payout scripts with zero values,
as a CompactSize-prefixed output vector. The payments come from the PPLNS
window anchored by the token, with the weights, cuts, rounding and script
order of peer payout validation (`PreparedNotifyParams::declared_outputs`,
`build_pplns_outputs`), so a declarator needs the node's share-chain
snapshot.

**Layout.** The opaque token carries the public commitment terms, integers
little endian. An address is `0` for none, or `1`, a network class byte and
the script (the share header's address encoding).

| Bytes | Content |
|---|---|
| 1 | Version, `5` |
| 32 | Random nonce, not part of the commitment |
| 32 | Previous share hash |
| CompactSize, then 32 each | Uncle count (at most 3) and uncle hashes |
| 4 | Share bits, compact |
| 4 | Share time |
| 1, then an address or nothing | Donation address |
| 2 | Donation in basis points |
| same as the donation | Fee address, then the fee in basis points |
| 8 | Share version and desired version, two `u32` ([Share validation](sharechain/validation.md#share-versions-follow-a-vote)) |
| 4 | Band cap ([The cap `C`](sharechain/small-miners-design.md#the-cap-c)) |
| 1 | Payout level, 1 to 7 ([Payout level](miner-options.md#payout-level)) |
| 4 | Weak ratio ([The weak target](sharechain/small-miners-design.md#the-weak-target)) |

A token is at most 255 bytes. Another version, more than 3 uncles, a payout
level outside 1 to 7, or bytes left over is not a token. Earlier layouts:
[Rule history](sharechain/rule-history.md).
`ShareCommitment::from_mining_job_token` decodes the terms: a declared
share's bits are both its minimum and its wanted bits, and it carries no
weak shares. Parsing alone does not authenticate a token.

**Lifetime.** The node accepts only a token in its registry, for 120
seconds after allocation, and consumes it once when a Mining connection
installs it. Full-Template success rotates the nonce, not the terms. A
token leaves the registry when its Job Declaration connection closes. A
connection holds at most 64 tokens: one more allocation is answered and
drops the oldest open token (neither declared nor used), or the oldest of
any state when none is open, so the connection is never closed for it. A
message that names a dropped token gets `invalid-mining-job-token`. The
registry holds 4,096 tokens for all connections; an allocation past that
closes its connection.

## The declared coinbase

The miner supplies its payout address, its share owner (a Bitcoin Cash
P2PKH share address) and the template's revenue. The share commitment binds
the raw scriptSig prefix and the merge-mining v2 commitment (root, tree
height `h` and salt; [Auxiliary work proofs](bch-auxpow.md)). The coinbase
is fixed:

- **scriptSig:** the prefix, beginning with the canonical height push, then
  the channel's 12-byte extranonce, within the 100-byte scriptSig limit;
- **version** 2, input sequence `0xfffffffe`, lock time `height - 1`;
- **outputs:** the 53-byte merge-mining commitment as output 0 (the exact
  empty commitment when no token is merge-mined), the exact PPLNS outputs,
  canonical padding, then the share commitment. Extra outputs and
  reordering are refused.

The PPLNS outputs follow the [hybrid payout rule](architecture/bch-hybrid-payouts.md)
at the coinbase limit of a token share or a no-token share, as output 0
makes the share.

## Rules a declared job must meet

The Mining connection checks each `SetCustomMiningJob`. A refusal is
`SetCustomMiningJob.Error` `invalid-custom-job`, with the reason in the
node's log, unless a code is given.

1. The connection negotiated work selection (`work-selection-not-negotiated`),
   the channel is extended (`invalid-channel-type`), and the node mines
   Bitcoin Cash in P2Pool mode (`unsupported-pool-mode`).
2. The token is in the registry and unused (`invalid-mining-job-token`).
3. The channel chose the token's donation (`donation-mismatch`).
4. The header has the previous block hash and nBits of the token's
   template, changes version bits only inside the node's version mask, and
   has `min_ntime` at or after the template's `mintime` and at most 7,200
   seconds ahead.
5. The coinbase is as above: version, sequence, lock time, a Bitcoin Cash
   P2PKH share owner, a valid output 0, a value within BCH's money range,
   and outputs equal to the node's rebuild byte for byte, within the
   coinbase limit. Its parts around the extranonce are at most 65,535
   bytes each.
6. The merkle path has at most 32 siblings. In Full-Template mode the
   coinbase is the declared prefix and suffix, the path is the one of the
   declared transactions in CTOR, and the template passes the node's BCH
   checks and block size limit.
7. A token job (output 0 not empty) carries the [declared token
   leaves](#declared-token-leaves), and its header nBits pass the
   [ASERT header check](bch-auxpow.md#the-asert-header-check)
   (`invalid-token-asert`).
8. A Full-Template job's block passes the node's proposal check, below.

### Full-Template

The client declares the serialized coinbase prefix and suffix around the
extranonce, reveals all non-coinbase transactions, then presents the
returned token in `SetCustomMiningJob`. The node orders the transactions by
CTOR, checks the merkle path and the coinbase against the declaration, and
asks its BCHN node to validate a block proposal without PoW. Only a null
proposal result installs the job. The success carries the installed channel
job id, and shares can be submitted at once. The node can broadcast a found
block in this mode.

A proposal the node does not accept gets one of three codes. The first two
do not say the template is bad: the client declares again and stays on the
pool.

| Code | When |
|---|---|
| `stale-chain-tip` | The node has left the block's parent at a Bitcoin Cash tip change and has no verdict (BCHN and BCHD answer `inconclusive-not-best-prevblk`), or the job's parent is not the pool template's |
| `template-unavailable` | The check got no answer within 10 seconds, or the request to the node failed |
| `invalid-template` | The node's own reason for refusing the block, such as `bad-cb-amount` |

A connection holds at most eight pending declarations. Revealed
transactions arrive in `ProvideMissingTransactions.Success`. A template past
65,535 transactions or one 16 MiB frame needs the
[large templates extension](architecture/sv2-large-templates-extension.md).
Declared transactions, in all connections together, may take two blocks at
the published template's size limit, and at least 64 MiB.

**PushSolution.** A Full-Template client that finds a block also sends
`PushSolution` on its Job Declaration connection (spec 6.4.9). The
connection keeps its last four Full-Template declarations on the current
previous block, under a second memory bound of the same size. The node
rebuilds the block from the declaration, the pushed extranonce and the
declared transactions in CTOR, checks the header against the token's
template target, and submits it to BCHN and every `[[submit_nodes]]` entry
(BCHN answers `duplicate` when the share also came on the Mining
connection). A `PushSolution` that solves none of the connection's
declarations, such as a Coinbase-Only client's, is ignored; none is
answered.

### Declared token leaves

A Full-Template declaration whose output 0 is a token root discloses its
leaves in `DeclareMiningJob.excess_data`, in Pickaxe's `CTMD` v2
([Disclosure to miners](bch-auxpow.md#disclosure-to-miners); pickaxe-miner
PR #70 at `a92fef7`, "Job Declaration leaves"): exactly one canonical
disclosure, nothing trailing, each split hashing to its leaf's `ext`, and
the leaves alone rebuilding output 0's root at its height and salt. Empty
`excess_data` means no root. A root without leaves, leaves without a root,
or leaves that do not decode or make the root get `DeclareMiningJob.Error`
`invalid-coinbase-tx`, the reason in its details. At installation each
leaf must pay the channel's miner (`payout_hash` is `HASH256` of the
miner's payout script) and carry the job's split: the token's donation,
then the node's fee, each to the script the coinbase pays it. So the fee
and the donation apply to declared tokens as to BCH, and a token whose
covenant takes no split cannot be merge-mined this way on a node that takes
either cut.

### Coinbase-Only

A Coinbase-Only client does not declare or reveal its transactions. It
presents the token with its coinbase and private merkle path directly. The
node checks the visible coinbase and header rules, the exact payments and
the share PoW. It cannot check hidden transactions, CTOR, actual fees or
the hidden block size. The declarator must validate and broadcast the
complete block; the node never broadcasts an incomplete substitute. A
Coinbase-Only job declares no leaves, so it merge-mines no token: its
output 0 must be the exact empty commitment.

### Which clients may use Coinbase-Only

A Coinbase-Only declarator's shares are credited on a block whose
transactions the node never sees (`block_available = false`). A declarator
that never lets a block through costs the other miners their part of it:
block withholding, not theft (sv2-spec 6.3.1; Delving Bitcoin "PPLNS with
job declaration"; jtoomim/p2pool#21). So a node takes Coinbase-Only only
from the IP addresses or CIDR ranges in `[stratum] sv2_coinbase_only_from`,
by default this computer. Add your own declarator, such as the Tailscale
address of your own Knuth node. `[]` refuses everyone and
`["0.0.0.0/0", "::/0"]` takes everyone; an IPv4-mapped IPv6 address counts
as its IPv4 address. Full-Template is taken from every client, since the
node checks the template. A refused client gets `SetupConnection.Error`
`unsupported-feature-flags` with `flags` set to `DECLARE_TX_DATA` (`0x1`),
and may set up again in Full-Template mode on the same connection.

Behind a Tor onion service or an I2P server tunnel
(`HiddenServicePort 3336 127.0.0.1:3336` in `torrc`), every miner connects
from this computer's own address and would get Coinbase-Only: set
`sv2_coinbase_only_from = []`, or point the onion service at an address of
this computer that is not in the list. Tokens and declared jobs are counted
per connection, not per address, so miners behind one onion service do not
use up each other's.

## Installed jobs

A channel keeps its last eight declared jobs, counted apart from the 32 of
the pool's own jobs it keeps: the pool still sends the channel its own job
at every share-chain move, and those do not push a declared job out. A
declared job stays until eight newer declared jobs are installed, until its
Bitcoin Cash parent is two blocks back, or until the job tracker drops it,
which keeps a job for at least 15 minutes. A share on a job that is gone
gets `stale-share`.

Once a job is active, its target and credited difficulty stay fixed.
Vardiff and `UpdateChannel` change later jobs. Device maximum targets apply
when each job is assigned; zero maximum targets are refused.

## Read-only preparation for an external declarator

`POST /jd_prepare` uses the normal REST API authentication. It accepts at
most 8192 JSON bytes and refuses unknown request fields. Schema version 1
contains `token` (hex), `payout` (CashAddr), `share_owner` (the
independently specified BCH share address), `height`, `nbits` (integer
compact bits), `previous_hash` (BCH RPC display order), `coinbase_value`
(satoshis), `script_prefix` (hex), and `auxiliary` (`root` as 32 byte
integers, `height` and `salt`, the merge-mining v2 commitment). It receives
no RPC credentials, private keys or transaction bytes.

It requires the current mining-base anchor and BCH template tip, weights
the PPLNS window by the paying block's difficulty (one cached snapshot,
keyed by anchor and bits), and has no fallback recipient for empty or
unavailable payout history. The node's own PPLNS and coinbase builders
produce the bytes. The response contains `version`, `token_hash` (SHA256),
`anchor`, `height`, `coinbase_value`, `coinbase_prefix`, `coinbase_suffix`
and `extranonce_size` (12). A malformed request gets 400; an unavailable or
mismatched snapshot or tip gets 503, and the caller retries with a new
token after synchronization. The endpoint neither authenticates nor
consumes SV2 tokens: the SV2 protocols still check the token, the
recipient, the outputs and the custom job.

## Payout outputs on the Job Declaration connection

With the extension `0x5052`, "Requested Payout Outputs", a standard Job
Declaration client asks its Job Declaration connection for each job's exact
coinbase outputs, without `/jd_prepare` and without knowing how P2Pool
builds a coinbase. A Bitcoin Cash node grants it to any Job Declaration
connection that asks. `AllocateMiningJobToken.user_identifier` is then the
channel's whole identity, and the job is checked as any other. Messages,
rules and errors: [Requested payout outputs](architecture/sv2-payout-extension.md).

## Tests

### Encrypted network regression

`encrypted_bch_job_declaration_modes_install_mine_and_emit` runs the real
listener with an authority key and three loopback connections (Mining,
Full-Template, Coinbase-Only) through the reference SV2 codec. It installs
both jobs, mines headers, checks both coinbase proofs and refuses a
consumed token. Its frames split mid-message, and its 3,830-byte
`excess_data` (sixteen token leaves) spans the listener's 4,096-byte reads.
`a_standard_client_mines_on_requested_payout_outputs_in_both_modes` does the
same with jobs built from `0x5052` replies alone. BCHN is mocked.

```sh
cargo nextest run -p p2poolv2_lib -E 'test(encrypted_bch_job_declaration_modes_install_mine_and_emit) | test(a_standard_client_mines_on_requested_payout_outputs_in_both_modes)'
```

### Two-node BCHN regtest check

<a id="encrypted-bchn-and-safa-check-2026-10-17"></a>

The ignored `bchn_declared_jobs_reach_both_real_nodes_over_noise` test runs
the same three Noise connections against two disposable, connected BCHN
nodes. The pool node checks and submits the Full-Template block (two token
leaves disclosed); the client broadcasts the Coinbase-Only block, which
carries a private transaction absent from both mempools. Both nodes must
accept each block, both coinbase proofs must match, and BCHN must refuse an
extra satoshi of coinbase value.
`bchn_jobs_on_requested_payout_outputs_reach_both_real_nodes` does the same
with jobs built from `0x5052` replies. A SAFA v1 withdrawal on such nodes:
[the SAFA evidence record](../p2poolv2_safa/README.md#regtest-evidence-and-reproduction).

Use fresh data directories outside the repository. The first node needs
this `bitcoin.conf`:

```ini
regtest=1
server=1
discover=0
dnsseed=0
listenonion=0
rpcuser=p2pool-jd-test
rpcpassword=local-regtest-only
rpcallowip=127.0.0.1
[regtest]
rpcbind=127.0.0.1
rpcport=19453
bind=127.0.0.1
port=19454
```

The second uses the same settings with RPC port 19455 and P2P port 19456;
add `connect=127.0.0.1:19454` under `[regtest]` and `disablewallet=1` before
that section. The fixed credentials are only for these loopback test nodes.
Start both nodes, create a wallet named `jd-regtest` on the first, and mine
at least 101 blocks to it. In PowerShell, with `$bchnBin` pointing to
BCHN's `bin` directory and `$clientData` and `$poolData` to the fresh
directories:

```powershell
Start-Process "$bchnBin/bitcoind.exe" -ArgumentList "`"-datadir=$clientData`"" -WindowStyle Hidden
Start-Process "$bchnBin/bitcoind.exe" -ArgumentList "`"-datadir=$poolData`"" -WindowStyle Hidden
# Wait until both RPC endpoints respond, then initialize the disposable wallet.
& "$bchnBin/bitcoin-cli.exe" "-datadir=$clientData" createwallet jd-regtest
$testAddress = & "$bchnBin/bitcoin-cli.exe" "-datadir=$clientData" -rpcwallet=jd-regtest getnewaddress
& "$bchnBin/bitcoin-cli.exe" "-datadir=$clientData" generatetoaddress 101 $testAddress
$env:BCHN_JD_CLIENT_RPC = 'http://127.0.0.1:19453'
$env:BCHN_JD_POOL_RPC = 'http://127.0.0.1:19455'
cargo nextest run -p p2poolv2_lib --run-ignored ignored-only -E 'test(bchn_declared_jobs_reach_both_real_nodes_over_noise)' --nocapture
# Stop only these two disposable nodes when finished.
& "$bchnBin/bitcoin-cli.exe" "-datadir=$clientData" stop
& "$bchnBin/bitcoin-cli.exe" "-datadir=$poolData" stop
```

Run the `0x5052` test the same way with its own name. The tests check
loopback endpoints and `chain = regtest` before spending, and need an
otherwise empty mempool and mature balance for two 1-BCH payments; run one
at a time against a pair. Both use this repository's own client, not
independent declarator or ASIC firmware.

### On a running node

The ignored `a_running_node_serves_requested_payout_outputs` test is a Job
Declaration client for a node that is already running. It needs no RPC
login, only the node's Stratum V2 address, its published authority key and
a mining identity:

```sh
P2POOL_SV2_ADDRESS=127.0.0.1:3336 \
P2POOL_SV2_AUTHORITY_KEY=9b... \
P2POOL_SV2_IDENTITY='bchtest:q....worker,p2p=tp2pool:q...' \
cargo test -p p2poolv2_lib --lib a_running_node_serves_requested_payout_outputs -- --ignored --nocapture
```

It declares an empty block on the node's own template and submits no
shares. It checks that a request for a block the node is not building on is
refused, that the outputs pay out the whole value with `EXACT_OUTPUTS` and
lock time `height - 1`, that a Full-Template job on them is installed after
the node's BCHN proposal check, and that the next share or block brings
`PayoutOutputsStale` and a fresh token the new outputs.
