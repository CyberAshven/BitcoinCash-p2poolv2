# Lean direct split measurements: version 2, share-chain order

Pinned libauth be3ed16396c9eb9c0e57343474d36079a37c8dae, BCH 2026. Program 818 bytes. All listed spends verify both complete VMs. Equal gross debt 20*C, C=2^32 raw tokens, multiplier 20, maximum 35-byte P2SH32 payee scripts, P2PKH submitter/change, one 100-byte signed P2PKH funding unlock, 700 inherited satoshis, P2SH20 root. All payee/submitter token amounts use nine-byte CompactSize. Fee=serialized bytes at 1 sat/byte.

| N | R bytes | Unlock bytes | Op cost/limit | Hash std; nonstd | Tx bytes/fee | Fee/payee | Dust payee/submitter/change | Gross front sats | Net external sats | Max item | Peak slots |
|---:|---:|---:|---:|---|---:|---:|---|---:|---:|---:|---:|
| 1 (35) | 952 | 981 | 87659/817600; 85483/817600 | 17/511; 17/3577 | 1373 | 1373.000 | 705 / 675 / 546 | 2753 | 2053 | 981 | 19 |
| 8 (35) | 1288 | 1317 | 316591/1086400; 313775/1086400 | 22/679; 22/4753 | 2318 | 289.750 | 705 / 675 / 546 | 8633 | 7933 | 1317 | 26 |
| 32 (35) | 2440 | 2469 | 1101799/2008000; 1096679/2008000 | 40/1255; 40/8785 | 5558 | 173.688 | 705 / 675 / 546 | 28793 | 28093 | 2469 | 50 |
| 64 (35) | 3976 | 4005 | 2148854/3236800; 2140662/3236800 | 64/2023; 64/14161 | 9878 | 154.344 | 705 / 675 / 546 | 55673 | 54973 | 4005 | 82 |
| 100 (35) | 5704 | 5733 | 3326882/4619200; 3315234/4619200 | 91/2887; 91/20209 | 14738 | 147.380 | 705 / 675 / 546 | 85913 | 85213 | 5733 | 118 |
| 188 (35) | 9928 | 9957 | 6208186/7998400; 6188090/7998400 | 157/4999; 157/34993 | 26618 | 141.585 | 705 / 675 / 546 | 159833 | 159133 | 9957 | 206 |

Largest N with 35-byte payees: 188. N=189 fails: lean redeem or unlocking bytecode exceeds 10000 bytes.

| N | R bytes | Unlock bytes | Op cost/limit | Hash std; nonstd | Tx bytes/fee | Fee/payee | Dust payee/submitter/change | Gross front sats | Net external sats | Max item | Peak slots |
|---:|---:|---:|---:|---|---:|---:|---|---:|---:|---:|---:|
| 238 (25) | 9948 | 9977 | 7790336/8014400; 7770240/8014400 | 157/5009; 157/35063 | 28608 | 120.202 | 675 / 675 / 546 | 189933 | 189233 | 9977 | 256 |

Largest N with 25-byte payees: 238. N=239 fails: lean redeem or unlocking bytecode exceeds 10000 bytes.

| N | R bytes | Unlock bytes | Op cost/limit | Hash std; nonstd | Tx bytes/fee | Fee/payee | Dust payee/submitter/change | Gross front sats | Net external sats | Max item | Peak slots |
|---:|---:|---:|---:|---|---:|---:|---|---:|---:|---:|---:|
| 198 (23) | 8032 | 8061 | 6480498/6481600; 6464242/6481600 | 127/4051; 127/28357 | 23216 | 117.253 | 669 / 675 / 546 | 156353 | 155653 | 8061 | 216 |

Largest N with 23-byte payees: 198. N=199 fails: Unable to verify transaction: error in evaluating input index 0: Program attempted an operation that would exceed the operation cost density limit. Maximum operation cost: 6510400 (density control length: 8138); operation cost following operation: 6510436..


Densest mixed standard-script profile maximum N=244: R=9904, unlock=9933, op=7978890/7979200, hash=156/4987, tx/fee=28754, fee/payee=117.844, grossfront=193313, netexternal=192613. Next N fails Unable to verify transaction: error in evaluating input index 0: Program attempted an operation that would exceed the operation cost density limit. Maximum operation cost: 7979200 (density control length: 9974); operation cost following operation: 7979208..

Method: lengths from actual R and input unlocking; final libauth counters and limits plus full opcode-debug trace maxima via existing vm::measure_input; transaction bytes from libauth BCH serializer after fixed-length Schnorr funding signature; dust=3*(serialized output bytes+148) via existing output_dust. Gross fronting=sum token-output BCH minima+fee; net external subtracts inherited 700, returned BCH change is excluded. The covenant minima conservatively reserve nine-byte token amounts (705 sats for P2SH32, 675 P2PKH); BCH-only change dust 546. 100000-byte standard transaction cap is checked by builder, 10000-byte unlock/redeem item caps checked before construction and by VMs. BCHN and market value are untested.
