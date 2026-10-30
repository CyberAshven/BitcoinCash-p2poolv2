# Lean direct split measurements

Pinned libauth be3ed16396c9eb9c0e57343474d36079a37c8dae, BCH 2026. Program 885 bytes. All listed spends verify both complete VMs. Equal gross debt 20*C, C=2^32 raw tokens, multiplier 20, maximum 35-byte P2SH32 payee scripts, P2PKH submitter/change, one 100-byte signed P2PKH funding unlock, 700 inherited satoshis, P2SH20 root. All payee/submitter token amounts use nine-byte CompactSize. Fee=serialized bytes at 1 sat/byte.

| N | R bytes | Unlock bytes | Op cost/limit | Hash std; nonstd | Tx bytes/fee | Fee/payee | Dust payee/submitter/change | Gross front sats | Net external sats | Max item | Peak slots |
|---:|---:|---:|---:|---|---:|---:|---|---:|---:|---:|---:|
| 1 (35) | 1019 | 1048 | 94060/871200; 91756/871200 | 18/544; 18/3811 | 1440 | 1440.000 | 705 / 675 / 546 | 2820 | 2120 | 1048 | 21 |
| 8 (35) | 1355 | 1384 | 363361/1140000; 360417/1140000 | 23/712; 23/4987 | 2385 | 298.125 | 705 / 675 / 546 | 8700 | 8000 | 1384 | 28 |
| 32 (35) | 2507 | 2536 | 1286977/2061600; 1281729/2061600 | 41/1288; 41/9019 | 5625 | 175.781 | 705 / 675 / 546 | 28860 | 28160 | 2536 | 52 |
| 64 (35) | 4043 | 4072 | 2518576/3290400; 2510256/3290400 | 65/2056; 65/14395 | 9945 | 155.391 | 705 / 675 / 546 | 55740 | 55040 | 4072 | 84 |
| 100 (35) | 5771 | 5800 | 3904216/4672800; 3892440/4672800 | 92/2920; 92/20443 | 14805 | 148.050 | 705 / 675 / 546 | 85980 | 85280 | 5800 | 120 |
| 187 (35) | 9947 | 9976 | 7254457/8013600; 7234361/8013600 | 157/5008; 157/35059 | 26550 | 141.979 | 705 / 675 / 546 | 159060 | 158360 | 9976 | 207 |

Largest N with 35-byte payees: 187. N=188 fails: lean redeem or unlocking bytecode exceeds 10000 bytes.

| N | R bytes | Unlock bytes | Op cost/limit | Hash std; nonstd | Tx bytes/fee | Fee/payee | Dust payee/submitter/change | Gross front sats | Net external sats | Max item | Peak slots |
|---:|---:|---:|---:|---|---:|---:|---|---:|---:|---:|---:|
| 99 (25) | 4733 | 4762 | 3838633/3842400; 3828905/3842400 | 76/2401; 76/16810 | 12690 | 128.182 | 675 / 675 / 546 | 80190 | 79490 | 4762 | 119 |

Largest N with 25-byte payees: 99. N=100 fails: Unable to verify transaction: error in evaluating input index 0: Program attempted an operation that would exceed the operation cost density limit. Maximum operation cost: 3872800 (density control length: 4841); operation cost following operation: 3872819..

| N | R bytes | Unlock bytes | Op cost/limit | Hash std; nonstd | Tx bytes/fee | Fee/payee | Dust payee/submitter/change | Gross front sats | Net external sats | Max item | Peak slots |
|---:|---:|---:|---:|---|---:|---:|---|---:|---:|---:|---:|
| 83 (23) | 3959 | 3988 | 3223099/3223200; 3215035/3223200 | 63/2014; 63/14101 | 10518 | 126.723 | 669 / 675 / 546 | 66720 | 66020 | 3988 | 103 |

Largest N with 23-byte payees: 83. N=84 fails: Unable to verify transaction: error in evaluating input index 0: Program attempted an operation that would exceed the operation cost density limit. Maximum operation cost: 3252000 (density control length: 4065); operation cost following operation: 3252022..


Densest mixed standard-script profile maximum N=207: R=9971, unlock=10000, op=7999555/8032800, hash=157/5020, tx/fee=27378, fee/payee=132.261, grossfront=171180, netexternal=170480. Next N fails Unable to verify transaction: error in evaluating input index 0: Program attempted an operation that would exceed the operation cost density limit. Maximum operation cost: 8032800 (density control length: 10041); operation cost following operation: 8032865..

Method: lengths from actual R and input unlocking; final libauth counters and limits plus full opcode-debug trace maxima via existing vm::measure_input; transaction bytes from libauth BCH serializer after fixed-length Schnorr funding signature; dust=3*(serialized output bytes+148) via existing output_dust. Gross fronting=sum token-output BCH minima+fee; net external subtracts inherited 700, returned BCH change is excluded. The covenant minima conservatively reserve nine-byte token amounts (705 sats for P2SH32, 675 P2PKH); BCH-only change dust 546. 100000-byte standard transaction cap is checked by builder, 10000-byte unlock/redeem item caps checked before construction and by VMs. BCHN and market value are untested.

## Additional capacity probes and scope

`cargo photon-lean-measure --small-capacity` scans the same near-10,000-byte
script mix with C=1 and equal debt 20, multiplier 10: standard capacity208.
`cargo photon-lean-measure --unequal-capacity` uses strictly descending debts
N-index+10: standard capacity209. These two generic covenant probes have
rewards below the current PHOTON claim encoding range and are not mineable
claim vectors.

`cargo photon-lean-measure --claim-capacity` sets the largest debt to 2^32,
with C=1, multiplier 10 and the remaining debts descending 218..11 at N = 209.
The total reward is 4,294,991,112 raw units, above the current2^32 amount floor.
This checks split amount compatibility only; it does not execute a claim.
All209 payees pass both complete VMs. R 9,971, unlock 10,000; operation cost
8,023,215/8,032,800 standard and 8,003,119/8,032,800 non-standard; hash iterations
157/5020 standard and 157/35143 non-standard. Transaction/fee 25,780 sats,
123.349 sats/payee; gross front 170,704 sats, net external 170,004 after inherited 700.
Peak stack+altstack slots 229; maximum observed item 10,000 bytes. N = 210 in this
same maximal-byte script mix fails operation cost in both modes.

The209 records are 86 P2SH20 scripts followed by 123 P2SH32 scripts. Exact dust
is 657 sats for the large first output,645 each for the other 85 P2SH20 payees,
681 each for 123 P2SH32 payees,651 for submitter and546 for BCH change. The
builder/covenant deliberately retain conservative nine-byte-amount minima:
669/705 for payees and675 for submitter, so these small-amount outputs receive
more BCH than exact dust. All of it remains with its recipient.

The fixed nine-byte-amount mix at N = 208 fails standard operation density but
passes non-standard mode; it is not a standard candidate. N = 207 is its largest
measured standard profile. 209 is the largest measured standard construction
in the amount-compatible boundary probes, not a proof of an absolute maximum
over every possible debt, multiplier, script mix, submitter, program
optimization or unresolved owner parameter. The exact global extremum remains
open. The mix maximizes R length under the 10,000-byte unlock limit with as many
35-byte scripts as fit, 25-byte scripts for remaining even-byte room, then 23-byte
scripts. The scanner increments N until the next whole transaction fails.
The100000-byte standard transaction ceiling is inactive in all reported rows.

The approximately110 sats/payee claim is not confirmed: the shortest uniform
standard profile here costs 126.723 sats/payee, the amount-compatible209-payee
probe 123.349, and the all 35-byte benchmark at100 payees148.050. A no-overhead
asymptote is 111 bytes for 23-byte scripts (36 bytes in R plus a 75-byte output);
shared contract/funding/submitter/change overhead and operation density still
apply. BCH dust fronting is additional to this fee.
