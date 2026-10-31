# Prototype measurements (phase 2a)

Pinned libauth: be3ed16396c9eb9c0e57343474d36079a37c8dae; BCH 2026.
Program SHA256: `5b6e4ea2802307e6fce8b94d8ed6d65751fc54af6370989688e39aa79b628f3c`.

All weights total 43,200 * 2^256 - 1, reward 2^63 - 1, scripts 35 bytes, four signed synthetic P2PKH funding inputs, P2PKH change. All transactions verify in both VM modes. Cost/hash pairs are standard; non-standard. Tx bytes equal fee in satoshis at 1 sat/byte. Dust is per ordered allocation output then change, using actual encoded amounts.

| Spend | L | Redeem | Program | Op cost/limit (std; nonstd) | Hash iterations/limit (std; nonstd) | Peak slots | Control depth | Max item | Tx bytes/fee | Dust sats |
|---|---:|---:|---:|---|---|---:|---:|---:|---:|---|
| Root P2SH20 | 2617 | 2414 | 2075 | 363534/2126400; 338830/2126400 | 193/1329; 193/9303 | 36 | 4 | 2617 | 3442 | 705, 705, 546 |
| Root P2SH32 | 2617 | 2414 | 2075 | 363665/2126400; 338961/2126400 | 193/1329; 193/9303 | 36 | 4 | 2617 | 3442 | 705, 705, 546 |
| Internal P2SH32 | 2617 | 2414 | 2075 | 363519/2126400; 338815/2126400 | 193/1329; 193/9303 | 36 | 4 | 2617 | 3442 | 705, 705, 546 |
| Terminal 8 P2SH32 | 3222 | 2414 | 2075 | 1038866/2610400; 1019026/2610400 | 155/1631; 155/11420 | 44 | 9 | 3222 | 4569 | 705, 705, 705, 705, 705, 705, 705, 705, 546 |
| Root P2SH20, 10,000-byte funders | 2617 | 2414 | 2075 | 403138/2126400; 378434/2126400 | 193/1329; 193/9303 | 36 | 4 | 10000 | 43050 | 705, 705, 546 |
| Internal P2SH32, 10,000-byte funders | 2617 | 2414 | 2075 | 403123/2126400; 378419/2126400 | 193/1329; 193/9303 | 36 | 4 | 10000 | 43050 | 705, 705, 546 |
| Terminal 8 P2SH32, 10,000-byte funders | 3222 | 2414 | 2075 | 1078470/2610400; 1058630/2610400 | 155/1631; 155/11420 | 44 | 9 | 10000 | 44177 | 705, 705, 705, 705, 705, 705, 705, 705, 546 |

| Leaves | Spends | Fees (sats) | External BCH (sats) | External BCH | First beneficiary path (sats) |
|---:|---:|---:|---:|---:|---:|
| 2 | 1 | 3446 | 4156 | 0.00004156 | 4156 |
| 8 | 1 | 4569 | 9509 | 0.00009509 | 9509 |
| 9 | 3 | 11271 | 16916 | 0.00016916 | 10088 |
| 64 | 15 | 60646 | 105066 | 0.00105066 | 21950 |
| 512 | 127 | 509262 | 869522 | 0.00869522 | 34391 |
| 4096 | 1023 | 4098190 | 6985170 | 0.06985170 | 46832 |

CPU HASH160(R): 100,000 hashes in 0.154738 s (1.547 microseconds/seed), this build on this host; no GPU implementation or GPU measurement.

Method: VM final metrics provide operation cost, hash iterations and their limits. Opcode debug traces provide maxima of stack+altstack+function_count, control_stack.len(), and byte length across both stacks and function bodies, including unlocking and redeem phases. Serialized BCH transactions provide bytes and actual token-prefix dust, 3*(output_bytes+148). Full-tree costs sum each constructed transaction's fee and allocation shortfall; inherited child BCH is passed into the next spend with distinct synthetic funding outpoints. Net outside cost equals final leaf BCH minus root's 700 sats plus fees. Path cost funds only the first canonical beneficiary's ancestors, including sibling outputs and the complete terminal bucket, with four P2PKH funding inputs and change per transaction. BCHN acceptance and group C are not tested.
