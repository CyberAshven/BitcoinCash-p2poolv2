# Seeded minimum-payout study: candidate rules

Candidate rules, not adopted: the queue is paid in the order miners' earned balances reached their minimum; the rest of each win is paid ahead to the miners whose shares already in the window are expected to earn the most, never beyond that expectation; a leftover too small for the next miner's minimum goes to the entry before it; no 4,096-script window stop; no launch history or custody. A list that cannot be built leaves that win unmined (strict). Seed `0x70686f746f6e6d70`; 5,000 potential pool wins per run; C=1,000,000 raw units; multiples uniform in 10/20/30/50; churn and delayed funding as in the written-rules study. Windows hold 12 hours of full-pool shares (4,320 at 10 s, 43,200 at 1 s), except the last three runs: 48 hours (17,280 at 10 s). BCH costs serialize each list through the version 2 (share-chain order) lean builder with 35-byte payees. Reproduce with `cargo photon-payout-sim --candidate-summary`; full per-period tables with `--candidate`.

### 1000 miners, giant70, Strict, N=64, share interval=10s

Run 145.612 days; history 0 wins / 0 raw units; blocked coverage 0, blocked clipped minimum 0; permanently held raw units 0 (includes historical custody assumption); outstanding fundable roots 0; first split opportunity Some(1).
Signed check: won 24255127589660 = gross paid 24255127589660 + unpaid 8354445718 - advanced 8354445718 + unallocated 0; reserved 0 is included in unpaid; reimbursement 221692000000 is included in gross paid. Outside claims 15697 emitted 76143896478847; ending vault 1987600975931493. Peak advanced 8755462895 raw units (1.805 mean rewards); advanced held by departed miners 0; paid beyond a miner's own bound 0.

| Period | Opportunities | Won | Splits | Payouts/opportunity | Payouts/won | Eligible rank>N mean/max | Fee/split sats | Gross front/split sats | Net external/split sats |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| first 100 | 100 | 100 | 99 | 44.720 | 44.720 | 0.00/0 | 7151.6 | 39672.6 | 38972.6 |
| later | 4900 | 4900 | 4901 | 44.331 | 44.331 | 0.00/1 | 7040.1 | 38961.8 | 38261.8 |

Waits pool completed payout intervals within class (event weighted); theory and solo columns average miners (miner weighted), so their ratio is not an unbiased speedup. Unpaid miners and zero-share miners are shown explicitly. Theoretical waits average each miner's m*C divided by its absolute expected raw-token earning rate. First payout waits start at study entry or actual joining. Reimbursement includes BCH dust and fee, rather than BCH miner fee alone.

| Hash class | Miners | Paid miners | Zero shares | Intervals | Mean wait d | P95 wait d | Mean solo d | Mean m*C/rate d | First payout mean d | Token reimbursement % | Unpaid raw units | Reserved raw units |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| giant | 1 | 1 | 0 | 4999 | 0.029 | 0.087 | 0.040 | 0.000 | 0.013 | 0.028 | 0 | 0 |
| high | 94 | 94 | 0 | 165720 | 0.070 | 0.197 | 13.363 | 0.077 | 0.147 | 3.035 | 2250561447 | 0 |
| medium | 257 | 257 | 0 | 50047 | 0.599 | 2.210 | 602.303 | 3.780 | 5.381 | 4.555 | 3780160352 | 0 |
| small | 246 | 116 | 91 | 457 | 13.166 | 46.152 | 57803.696 | 335.775 | 47.203 | 5.343 | 2220179447 | 0 |
| tiny | 242 | 1 | 236 | 0 | censored | censored | 6265193.915 | 33318.446 | 47.183 | 4.905 | 103544472 | 0 |
| very-tiny | 160 | 0 | 160 | 0 | censored | censored | 142787836.410 | 898741.641 | censored | censored | 0 | 0 |

Fixed absolute probe: hash fraction 0.00010000, m=20, payouts 124, mean/P95 interval 1.162/3.492 days, theoretical threshold 1.145 days, solo 277.778 days, unpaid 4381848 raw units.

### 1000 miners, log-uniform, Strict, N=64, share interval=10s

Run 163.270 days; history 0 wins / 0 raw units; blocked coverage 0, blocked clipped minimum 0; permanently held raw units 0 (includes historical custody assumption); outstanding fundable roots 0; first split opportunity Some(1).
Signed check: won 24199645707077 = gross paid 24199645707077 + unpaid 10828802732 - advanced 10828802732 + unallocated 0; reserved 0 is included in unpaid; reimbursement 319707000000 is included in gross paid. Outside claims 17760 emitted 85938290827742; ending vault 1977862063465181. Peak advanced 11184022200 raw units (2.311 mean rewards); advanced held by departed miners 0; paid beyond a miner's own bound 7651113633.

| Period | Opportunities | Won | Splits | Payouts/opportunity | Payouts/won | Eligible rank>N mean/max | Fee/split sats | Gross front/split sats | Net external/split sats |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| first 100 | 100 | 100 | 99 | 62.740 | 62.740 | 36.88/51 | 9536.0 | 54889.4 | 54189.4 |
| later | 4900 | 4900 | 4901 | 63.966 | 63.966 | 30.57/57 | 9611.8 | 55373.6 | 54673.6 |

Waits pool completed payout intervals within class (event weighted); theory and solo columns average miners (miner weighted), so their ratio is not an unbiased speedup. Unpaid miners and zero-share miners are shown explicitly. Theoretical waits average each miner's m*C divided by its absolute expected raw-token earning rate. First payout waits start at study entry or actual joining. Reimbursement includes BCH dust and fee, rather than BCH miner fee alone.

| Hash class | Miners | Paid miners | Zero shares | Intervals | Mean wait d | P95 wait d | Mean solo d | Mean m*C/rate d | First payout mean d | Token reimbursement % | Unpaid raw units | Reserved raw units |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| high | 150 | 150 | 0 | 261514 | 0.080 | 0.242 | 8.575 | 0.050 | 0.122 | 1.150 | 4450175158 | 0 |
| medium | 274 | 273 | 0 | 57331 | 0.635 | 2.338 | 587.205 | 3.414 | 4.749 | 4.014 | 4313084393 | 0 |
| small | 233 | 106 | 91 | 332 | 17.954 | 63.197 | 55337.249 | 336.595 | 48.398 | 4.621 | 1975272983 | 0 |
| tiny | 256 | 1 | 251 | 0 | censored | censored | 6026253.083 | 37236.560 | 132.209 | 8.253 | 90270198 | 0 |
| very-tiny | 87 | 0 | 87 | 0 | censored | censored | 66155785.076 | 414549.796 | censored | censored | 0 | 0 |

Fixed absolute probe: hash fraction 0.00010000, m=20, payouts 164, mean/P95 interval 0.987/3.026 days, theoretical threshold 1.148 days, solo 277.778 days, unpaid 16353911 raw units.

### 1000 miners, heavy-tail, Strict, N=64, share interval=10s

Run 164.003 days; history 0 wins / 0 raw units; blocked coverage 0, blocked clipped minimum 0; permanently held raw units 0 (includes historical custody assumption); outstanding fundable roots 0; first split opportunity Some(1).
Signed check: won 24195558981795 = gross paid 24195558981795 + unpaid 15286017104 - advanced 15286017104 + unallocated 0; reserved 0 is included in unpaid; reimbursement 319666000000 is included in gross paid. Outside claims 17817 emitted 86210781791049; ending vault 1977593659227156. Peak advanced 16634814532 raw units (3.438 mean rewards); advanced held by departed miners 0; paid beyond a miner's own bound 9465623549.

| Period | Opportunities | Won | Splits | Payouts/opportunity | Payouts/won | Eligible rank>N mean/max | Fee/split sats | Gross front/split sats | Net external/split sats |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| first 100 | 100 | 100 | 99 | 62.610 | 62.610 | 130.70/167 | 9518.8 | 54779.7 | 54079.7 |
| later | 4900 | 4900 | 4901 | 63.960 | 63.960 | 132.80/196 | 9611.1 | 55368.8 | 54668.8 |

Waits pool completed payout intervals within class (event weighted); theory and solo columns average miners (miner weighted), so their ratio is not an unbiased speedup. Unpaid miners and zero-share miners are shown explicitly. Theoretical waits average each miner's m*C divided by its absolute expected raw-token earning rate. First payout waits start at study entry or actual joining. Reimbursement includes BCH dust and fee, rather than BCH miner fee alone.

| Hash class | Miners | Paid miners | Zero shares | Intervals | Mean wait d | P95 wait d | Mean solo d | Mean m*C/rate d | First payout mean d | Token reimbursement % | Unpaid raw units | Reserved raw units |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| giant | 1 | 1 | 0 | 4999 | 0.033 | 0.098 | 0.172 | 0.001 | 0.014 | 0.111 | 0 | 0 |
| high | 143 | 143 | 0 | 116119 | 0.165 | 0.399 | 14.896 | 0.081 | 0.195 | 0.884 | 4022618058 | 0 |
| medium | 656 | 656 | 0 | 197748 | 0.448 | 1.287 | 77.410 | 0.462 | 0.820 | 3.043 | 11263399046 | 0 |
| tiny | 70 | 0 | 70 | 0 | censored | censored | 9146399.273 | 63173.619 | censored | censored | 0 | 0 |
| very-tiny | 130 | 0 | 130 | 0 | censored | censored | 2232137981.665 | 12971277.301 | censored | censored | 0 | 0 |

Fixed absolute probe: hash fraction 0.00010000, m=20, payouts 145, mean/P95 interval 1.123/3.492 days, theoretical threshold 1.148 days, solo 277.778 days, unpaid 0 raw units.

### 10000 miners, giant70, Strict, N=64, share interval=10s

Run 145.565 days; history 0 wins / 0 raw units; blocked coverage 80, blocked clipped minimum 0; permanently held raw units 0 (includes historical custody assumption); outstanding fundable roots 0; first split opportunity Some(1).
Signed check: won 23874175478593 = gross paid 23874175478593 + unpaid 35194272459 - advanced 35194272459 + unallocated 0; reserved 0 is included in unpaid; reimbursement 230898000000 is included in gross paid. Outside claims 15714 emitted 76226687355047; ending vault 1987899137166360. Peak advanced 36103275909 raw units (7.440 mean rewards); advanced held by departed miners 46467177; paid beyond a miner's own bound 42128280741.

| Period | Opportunities | Won | Splits | Payouts/opportunity | Payouts/won | Eligible rank>N mean/max | Fee/split sats | Gross front/split sats | Net external/split sats |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| first 100 | 100 | 100 | 99 | 56.350 | 56.350 | 4.28/31 | 8690.7 | 49493.7 | 48793.7 |
| later | 4900 | 4820 | 4821 | 45.972 | 46.735 | 0.68/35 | 7355.0 | 40971.4 | 40271.4 |

Waits pool completed payout intervals within class (event weighted); theory and solo columns average miners (miner weighted), so their ratio is not an unbiased speedup. Unpaid miners and zero-share miners are shown explicitly. Theoretical waits average each miner's m*C divided by its absolute expected raw-token earning rate. First payout waits start at study entry or actual joining. Reimbursement includes BCH dust and fee, rather than BCH miner fee alone.

| Hash class | Miners | Paid miners | Zero shares | Intervals | Mean wait d | P95 wait d | Mean solo d | Mean m*C/rate d | First payout mean d | Token reimbursement % | Unpaid raw units | Reserved raw units |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| giant | 1 | 1 | 0 | 4919 | 0.030 | 0.088 | 0.040 | 0.000 | 0.056 | 0.028 | 0 | 0 |
| medium | 2195 | 2190 | 0 | 219143 | 1.161 | 4.244 | 691.375 | 3.971 | 5.650 | 3.467 | 17236289842 | 0 |
| small | 2520 | 1119 | 1022 | 3499 | 15.559 | 60.731 | 58740.636 | 329.292 | 49.073 | 4.709 | 17187333201 | 0 |
| tiny | 2460 | 21 | 2405 | 6 | 0.207 | 0.286 | 5896173.020 | 33512.020 | 83.254 | 7.379 | 754961751 | 0 |
| very-tiny | 2824 | 0 | 2823 | 0 | censored | censored | 947397972.751 | 5219793.796 | censored | censored | 15687665 | 0 |

Fixed absolute probe: hash fraction 0.00010000, m=20, payouts 92, mean/P95 interval 1.538/4.043 days, theoretical threshold 1.145 days, solo 277.778 days, unpaid 18490232 raw units.

### 10000 miners, log-uniform, Strict, N=64, share interval=10s

Run 162.130 days; history 0 wins / 0 raw units; blocked coverage 4927, blocked clipped minimum 0; permanently held raw units 0 (includes historical custody assumption); outstanding fundable roots 0; first split opportunity Some(1).
Signed check: won 357090798141 = gross paid 357090798141 + unpaid 38044882807 - advanced 38044882807 + unallocated 0; reserved 0 is included in unpaid; reimbursement 4615000000 is included in gross paid. Outside claims 17634 emitted 85841700570322; ending vault 2001801208631537. Peak advanced 39247927605 raw units (8.023 mean rewards); advanced held by departed miners 3347042891; paid beyond a miner's own bound 329299215.

| Period | Opportunities | Won | Splits | Payouts/opportunity | Payouts/won | Eligible rank>N mean/max | Fee/split sats | Gross front/split sats | Net external/split sats |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| first 100 | 100 | 20 | 20 | 12.570 | 62.850 | 338.42/410 | 9467.4 | 54451.6 | 53751.6 |
| later | 4900 | 53 | 53 | 0.685 | 63.358 | 496.06/533 | 9534.0 | 54876.7 | 54176.7 |

Waits pool completed payout intervals within class (event weighted); theory and solo columns average miners (miner weighted), so their ratio is not an unbiased speedup. Unpaid miners and zero-share miners are shown explicitly. Theoretical waits average each miner's m*C divided by its absolute expected raw-token earning rate. First payout waits start at study entry or actual joining. Reimbursement includes BCH dust and fee, rather than BCH miner fee alone.

| Hash class | Miners | Paid miners | Zero shares | Intervals | Mean wait d | P95 wait d | Mean solo d | Mean m*C/rate d | First payout mean d | Token reimbursement % | Unpaid raw units | Reserved raw units |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| high | 318 | 316 | 0 | 1618 | 23.061 | 78.206 | 20.784 | 0.118 | 2.534 | 1.108 | 3507751465 | 0 |
| medium | 2520 | 1254 | 0 | 1421 | 48.945 | 135.698 | 587.498 | 3.368 | 42.774 | 1.466 | 32717072334 | 0 |
| small | 2460 | 4 | 892 | 2 | 2.921 | 3.334 | 56170.578 | 319.381 | 14.816 | 4.490 | 1803174639 | 0 |
| tiny | 2568 | 0 | 2503 | 0 | censored | censored | 6109279.423 | 33971.620 | censored | censored | 16884369 | 0 |
| very-tiny | 2134 | 0 | 2131 | 0 | censored | censored | 370672804.550 | 2034764.422 | censored | censored | 0 | 0 |

Fixed absolute probe: hash fraction 0.00010000, m=20, payouts 1, mean/P95 interval censored/censored days, theoretical threshold 1.141 days, solo 277.778 days, unpaid 7754209 raw units.

### 10000 miners, heavy-tail, Strict, N=64, share interval=10s

Run 180.020 days; history 0 wins / 0 raw units; blocked coverage 4980, blocked clipped minimum 0; permanently held raw units 0 (includes historical custody assumption); outstanding fundable roots 0; first split opportunity Some(1).
Signed check: won 98171588234 = gross paid 98171588234 + unpaid 33554951579 - advanced 33554951579 + unallocated 0; reserved 0 is included in unpaid; reimbursement 1221000000 is included in gross paid. Outside claims 19450 emitted 94485762729410; ending vault 1993416065682356. Peak advanced 33554951579 raw units (6.836 mean rewards); advanced held by departed miners 1888243157; paid beyond a miner's own bound 263317774.

| Period | Opportunities | Won | Splits | Payouts/opportunity | Payouts/won | Eligible rank>N mean/max | Fee/split sats | Gross front/split sats | Net external/split sats |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| first 100 | 100 | 9 | 9 | 5.220 | 58.000 | 295.51/322 | 8832.0 | 50397.0 | 49697.0 |
| later | 4900 | 11 | 11 | 0.143 | 63.545 | 325.76/341 | 9558.5 | 55033.0 | 54333.0 |

Waits pool completed payout intervals within class (event weighted); theory and solo columns average miners (miner weighted), so their ratio is not an unbiased speedup. Unpaid miners and zero-share miners are shown explicitly. Theoretical waits average each miner's m*C divided by its absolute expected raw-token earning rate. First payout waits start at study entry or actual joining. Reimbursement includes BCH dust and fee, rather than BCH miner fee alone.

| Hash class | Miners | Paid miners | Zero shares | Intervals | Mean wait d | P95 wait d | Mean solo d | Mean m*C/rate d | First payout mean d | Token reimbursement % | Unpaid raw units | Reserved raw units |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| giant | 1 | 1 | 0 | 5 | 0.336 | 1.057 | 0.241 | 0.001 | 0.123 | 0.079 | 0 | 0 |
| high | 109 | 109 | 0 | 320 | 24.673 | 124.262 | 15.628 | 0.085 | 1.962 | 0.897 | 853283648 | 0 |
| medium | 7890 | 677 | 0 | 109 | 53.810 | 128.554 | 730.158 | 4.101 | 54.333 | 1.838 | 32701667931 | 0 |
| tiny | 153 | 0 | 152 | 0 | censored | censored | 19962327.532 | 109942.068 | censored | censored | 0 | 0 |
| very-tiny | 1847 | 0 | 1846 | 0 | censored | censored | 16415371799.757 | 93244060.308 | censored | censored | 0 | 0 |

Fixed absolute probe: hash fraction 0.00010000, m=20, payouts 0, mean/P95 interval censored/censored days, theoretical threshold 1.144 days, solo 277.778 days, unpaid 5719776 raw units.

### 1000 miners, giant70, Strict, N=187, share interval=10s

Run 145.612 days; history 0 wins / 0 raw units; blocked coverage 0, blocked clipped minimum 0; permanently held raw units 0 (includes historical custody assumption); outstanding fundable roots 0; first split opportunity Some(1).
Signed check: won 24255127589660 = gross paid 24255127589660 + unpaid 8354445718 - advanced 8354445718 + unallocated 0; reserved 0 is included in unpaid; reimbursement 221692000000 is included in gross paid. Outside claims 15697 emitted 76143896478847; ending vault 1987600975931493. Peak advanced 8755462895 raw units (1.805 mean rewards); advanced held by departed miners 0; paid beyond a miner's own bound 0.

| Period | Opportunities | Won | Splits | Payouts/opportunity | Payouts/won | Eligible rank>N mean/max | Fee/split sats | Gross front/split sats | Net external/split sats |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| first 100 | 100 | 100 | 99 | 44.720 | 44.720 | 0.00/0 | 7151.6 | 39672.6 | 38972.6 |
| later | 4900 | 4900 | 4901 | 44.331 | 44.331 | 0.00/0 | 7040.1 | 38961.8 | 38261.8 |

Waits pool completed payout intervals within class (event weighted); theory and solo columns average miners (miner weighted), so their ratio is not an unbiased speedup. Unpaid miners and zero-share miners are shown explicitly. Theoretical waits average each miner's m*C divided by its absolute expected raw-token earning rate. First payout waits start at study entry or actual joining. Reimbursement includes BCH dust and fee, rather than BCH miner fee alone.

| Hash class | Miners | Paid miners | Zero shares | Intervals | Mean wait d | P95 wait d | Mean solo d | Mean m*C/rate d | First payout mean d | Token reimbursement % | Unpaid raw units | Reserved raw units |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| giant | 1 | 1 | 0 | 4999 | 0.029 | 0.087 | 0.040 | 0.000 | 0.013 | 0.028 | 0 | 0 |
| high | 94 | 94 | 0 | 165720 | 0.070 | 0.197 | 13.363 | 0.077 | 0.147 | 3.035 | 2250561447 | 0 |
| medium | 257 | 257 | 0 | 50047 | 0.599 | 2.210 | 602.303 | 3.780 | 5.381 | 4.555 | 3780160352 | 0 |
| small | 246 | 116 | 91 | 457 | 13.166 | 46.152 | 57803.696 | 335.775 | 47.203 | 5.343 | 2220179447 | 0 |
| tiny | 242 | 1 | 236 | 0 | censored | censored | 6265193.915 | 33318.446 | 47.183 | 4.905 | 103544472 | 0 |
| very-tiny | 160 | 0 | 160 | 0 | censored | censored | 142787836.410 | 898741.641 | censored | censored | 0 | 0 |

Fixed absolute probe: hash fraction 0.00010000, m=20, payouts 124, mean/P95 interval 1.162/3.492 days, theoretical threshold 1.145 days, solo 277.778 days, unpaid 4381848 raw units.

### 1000 miners, log-uniform, Strict, N=187, share interval=10s

Run 163.270 days; history 0 wins / 0 raw units; blocked coverage 0, blocked clipped minimum 0; permanently held raw units 0 (includes historical custody assumption); outstanding fundable roots 0; first split opportunity Some(1).
Signed check: won 24199645707077 = gross paid 24199645707077 + unpaid 9828603238 - advanced 9828603238 + unallocated 0; reserved 0 is included in unpaid; reimbursement 417837000000 is included in gross paid. Outside claims 17760 emitted 85938290827742; ending vault 1977862063465181. Peak advanced 10291568978 raw units (2.126 mean rewards); advanced held by departed miners 0; paid beyond a miner's own bound 13337591981.

| Period | Opportunities | Won | Splits | Payouts/opportunity | Payouts/won | Eligible rank>N mean/max | Fee/split sats | Gross front/split sats | Net external/split sats |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| first 100 | 100 | 100 | 99 | 87.810 | 87.810 | 0.00/0 | 12853.3 | 76059.7 | 75359.7 |
| later | 4900 | 4900 | 4901 | 83.481 | 83.481 | 0.00/0 | 12167.8 | 71684.7 | 70984.7 |

Waits pool completed payout intervals within class (event weighted); theory and solo columns average miners (miner weighted), so their ratio is not an unbiased speedup. Unpaid miners and zero-share miners are shown explicitly. Theoretical waits average each miner's m*C divided by its absolute expected raw-token earning rate. First payout waits start at study entry or actual joining. Reimbursement includes BCH dust and fee, rather than BCH miner fee alone.

| Hash class | Miners | Paid miners | Zero shares | Intervals | Mean wait d | P95 wait d | Mean solo d | Mean m*C/rate d | First payout mean d | Token reimbursement % | Unpaid raw units | Reserved raw units |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| high | 150 | 150 | 0 | 352262 | 0.060 | 0.202 | 8.575 | 0.050 | 0.115 | 1.549 | 3771772594 | 0 |
| medium | 274 | 273 | 0 | 64695 | 0.563 | 2.093 | 587.205 | 3.414 | 4.724 | 4.526 | 3917279151 | 0 |
| small | 233 | 106 | 91 | 350 | 16.810 | 60.926 | 55337.249 | 336.595 | 48.371 | 4.849 | 2048179819 | 0 |
| tiny | 256 | 1 | 251 | 0 | censored | censored | 6026253.083 | 37236.560 | 132.183 | 9.078 | 91371674 | 0 |
| very-tiny | 87 | 0 | 87 | 0 | censored | censored | 66155785.076 | 414549.796 | censored | censored | 0 | 0 |

Fixed absolute probe: hash fraction 0.00010000, m=20, payouts 164, mean/P95 interval 0.987/3.026 days, theoretical threshold 1.148 days, solo 277.778 days, unpaid 16353911 raw units.

### 1000 miners, heavy-tail, Strict, N=187, share interval=10s

Run 164.003 days; history 0 wins / 0 raw units; blocked coverage 0, blocked clipped minimum 0; permanently held raw units 0 (includes historical custody assumption); outstanding fundable roots 0; first split opportunity Some(1).
Signed check: won 24195558981795 = gross paid 24195558981795 + unpaid 12840406276 - advanced 12840406276 + unallocated 0; reserved 0 is included in unpaid; reimbursement 549414000000 is included in gross paid. Outside claims 17817 emitted 86210781791049; ending vault 1977593659227156. Peak advanced 14059318392 raw units (2.905 mean rewards); advanced held by departed miners 0; paid beyond a miner's own bound 12848494747.

| Period | Opportunities | Won | Splits | Payouts/opportunity | Payouts/won | Eligible rank>N mean/max | Fee/split sats | Gross front/split sats | Net external/split sats |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| first 100 | 100 | 100 | 99 | 111.020 | 111.020 | 0.00/0 | 15924.5 | 95659.2 | 94959.2 |
| later | 4900 | 4900 | 4901 | 109.860 | 109.860 | 0.00/0 | 15622.7 | 93732.9 | 93032.9 |

Waits pool completed payout intervals within class (event weighted); theory and solo columns average miners (miner weighted), so their ratio is not an unbiased speedup. Unpaid miners and zero-share miners are shown explicitly. Theoretical waits average each miner's m*C divided by its absolute expected raw-token earning rate. First payout waits start at study entry or actual joining. Reimbursement includes BCH dust and fee, rather than BCH miner fee alone.

| Hash class | Miners | Paid miners | Zero shares | Intervals | Mean wait d | P95 wait d | Mean solo d | Mean m*C/rate d | First payout mean d | Token reimbursement % | Unpaid raw units | Reserved raw units |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| giant | 1 | 1 | 0 | 4993 | 0.033 | 0.098 | 0.172 | 0.001 | 0.014 | 0.110 | 0 | 0 |
| high | 143 | 143 | 0 | 261380 | 0.073 | 0.225 | 14.896 | 0.081 | 0.170 | 1.988 | 2900902453 | 0 |
| medium | 656 | 656 | 0 | 282241 | 0.314 | 1.045 | 77.410 | 0.462 | 0.759 | 4.339 | 9939503823 | 0 |
| tiny | 70 | 0 | 70 | 0 | censored | censored | 9146399.273 | 63173.619 | censored | censored | 0 | 0 |
| very-tiny | 130 | 0 | 130 | 0 | censored | censored | 2232137981.665 | 12971277.301 | censored | censored | 0 | 0 |

Fixed absolute probe: hash fraction 0.00010000, m=20, payouts 164, mean/P95 interval 0.992/2.974 days, theoretical threshold 1.148 days, solo 277.778 days, unpaid 11991118 raw units.

### 10000 miners, giant70, Strict, N=187, share interval=10s

Run 145.565 days; history 0 wins / 0 raw units; blocked coverage 57, blocked clipped minimum 0; permanently held raw units 0 (includes historical custody assumption); outstanding fundable roots 0; first split opportunity Some(1).
Signed check: won 23983358316041 = gross paid 23983358316041 + unpaid 35138040831 - advanced 35138040831 + unallocated 0; reserved 0 is included in unpaid; reimbursement 232883000000 is included in gross paid. Outside claims 15714 emitted 76226362095438; ending vault 1987790279588521. Peak advanced 36009118266 raw units (7.422 mean rewards); advanced held by departed miners 25952979; paid beyond a miner's own bound 42649834714.

| Period | Opportunities | Won | Splits | Payouts/opportunity | Payouts/won | Eligible rank>N mean/max | Fee/split sats | Gross front/split sats | Net external/split sats |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| first 100 | 100 | 100 | 99 | 56.880 | 56.880 | 0.00/0 | 8760.8 | 49941.2 | 49241.2 |
| later | 4900 | 4843 | 4844 | 46.366 | 46.912 | 0.01/2 | 7378.2 | 41119.4 | 40419.4 |

Waits pool completed payout intervals within class (event weighted); theory and solo columns average miners (miner weighted), so their ratio is not an unbiased speedup. Unpaid miners and zero-share miners are shown explicitly. Theoretical waits average each miner's m*C divided by its absolute expected raw-token earning rate. First payout waits start at study entry or actual joining. Reimbursement includes BCH dust and fee, rather than BCH miner fee alone.

| Hash class | Miners | Paid miners | Zero shares | Intervals | Mean wait d | P95 wait d | Mean solo d | Mean m*C/rate d | First payout mean d | Token reimbursement % | Unpaid raw units | Reserved raw units |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| giant | 1 | 1 | 0 | 4942 | 0.029 | 0.087 | 0.040 | 0.000 | 0.056 | 0.028 | 0 | 0 |
| medium | 2195 | 2190 | 0 | 221062 | 1.152 | 4.223 | 691.375 | 3.971 | 5.648 | 3.481 | 17217839675 | 0 |
| small | 2520 | 1123 | 1022 | 3537 | 15.397 | 61.168 | 58740.636 | 329.293 | 49.251 | 4.714 | 17139647260 | 0 |
| tiny | 2460 | 21 | 2405 | 7 | 0.200 | 0.286 | 5896173.020 | 33512.167 | 83.289 | 8.129 | 764866231 | 0 |
| very-tiny | 2824 | 0 | 2823 | 0 | censored | censored | 947397972.751 | 5219816.758 | censored | censored | 15687665 | 0 |

Fixed absolute probe: hash fraction 0.00010000, m=20, payouts 92, mean/P95 interval 1.538/3.920 days, theoretical threshold 1.145 days, solo 277.778 days, unpaid 19584979 raw units.

### 10000 miners, log-uniform, Strict, N=187, share interval=10s

Run 162.130 days; history 0 wins / 0 raw units; blocked coverage 380, blocked clipped minimum 0; permanently held raw units 0 (includes historical custody assumption); outstanding fundable roots 0; first split opportunity Some(1).
Signed check: won 22394539122350 = gross paid 22394539122350 + unpaid 32964412698 - advanced 32964412698 + unallocated 0; reserved 0 is included in unpaid; reimbursement 562354000000 is included in gross paid. Outside claims 17634 emitted 85359120957369; ending vault 1980246339920281. Peak advanced 34089616227 raw units (7.033 mean rewards); advanced held by departed miners 106864916; paid beyond a miner's own bound 30653803473.

| Period | Opportunities | Won | Splits | Payouts/opportunity | Payouts/won | Eligible rank>N mean/max | Fee/split sats | Gross front/split sats | Net external/split sats |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| first 100 | 100 | 100 | 99 | 124.950 | 124.950 | 0.00/0 | 17767.8 | 107422.3 | 106722.3 |
| later | 4900 | 4520 | 4521 | 112.216 | 121.650 | 0.12/13 | 17166.7 | 103586.1 | 102886.1 |

Waits pool completed payout intervals within class (event weighted); theory and solo columns average miners (miner weighted), so their ratio is not an unbiased speedup. Unpaid miners and zero-share miners are shown explicitly. Theoretical waits average each miner's m*C divided by its absolute expected raw-token earning rate. First payout waits start at study entry or actual joining. Reimbursement includes BCH dust and fee, rather than BCH miner fee alone.

| Hash class | Miners | Paid miners | Zero shares | Intervals | Mean wait d | P95 wait d | Mean solo d | Mean m*C/rate d | First payout mean d | Token reimbursement % | Unpaid raw units | Reserved raw units |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| high | 318 | 318 | 0 | 218322 | 0.207 | 0.542 | 20.784 | 0.118 | 0.199 | 2.202 | 316703269 | 0 |
| medium | 2520 | 2519 | 0 | 336876 | 0.976 | 3.463 | 587.498 | 3.387 | 4.705 | 2.747 | 16853631501 | 0 |
| small | 2460 | 1211 | 892 | 3073 | 21.464 | 70.802 | 56170.578 | 321.177 | 50.927 | 3.948 | 15053892071 | 0 |
| tiny | 2568 | 31 | 2503 | 2 | 0.185 | 0.306 | 6109279.423 | 34162.666 | 78.903 | 5.696 | 705639825 | 0 |
| very-tiny | 2134 | 1 | 2131 | 1 | 0.133 | 0.133 | 370672804.550 | 2046207.342 | 93.917 | 9.958 | 34546032 | 0 |

Fixed absolute probe: hash fraction 0.00010000, m=20, payouts 109, mean/P95 interval 1.477/3.713 days, theoretical threshold 1.148 days, solo 277.778 days, unpaid 12894054 raw units.

### 10000 miners, heavy-tail, Strict, N=187, share interval=10s

Run 180.020 days; history 0 wins / 0 raw units; blocked coverage 1680, blocked clipped minimum 0; permanently held raw units 0 (includes historical custody assumption); outstanding fundable roots 0; first split opportunity Some(1).
Signed check: won 16065322886320 = gross paid 16065322886320 + unpaid 51300321939 - advanced 51300321939 + unallocated 0; reserved 0 is included in unpaid; reimbursement 400813000000 is included in gross paid. Outside claims 19450 emitted 94119707192747; ending vault 1977814969920933. Peak advanced 51548452127 raw units (10.653 mean rewards); advanced held by departed miners 3480959527; paid beyond a miner's own bound 18938988125.

| Period | Opportunities | Won | Splits | Payouts/opportunity | Payouts/won | Eligible rank>N mean/max | Fee/split sats | Gross front/split sats | Net external/split sats |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| first 100 | 100 | 93 | 93 | 124.960 | 134.366 | 1.17/43 | 18835.9 | 114238.6 | 113538.6 |
| later | 4900 | 3227 | 3227 | 79.248 | 120.334 | 0.51/14 | 16997.7 | 102508.0 | 101808.0 |

Waits pool completed payout intervals within class (event weighted); theory and solo columns average miners (miner weighted), so their ratio is not an unbiased speedup. Unpaid miners and zero-share miners are shown explicitly. Theoretical waits average each miner's m*C divided by its absolute expected raw-token earning rate. First payout waits start at study entry or actual joining. Reimbursement includes BCH dust and fee, rather than BCH miner fee alone.

| Hash class | Miners | Paid miners | Zero shares | Intervals | Mean wait d | P95 wait d | Mean solo d | Mean m*C/rate d | First payout mean d | Token reimbursement % | Unpaid raw units | Reserved raw units |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| giant | 1 | 1 | 0 | 1036 | 0.050 | 0.118 | 0.241 | 0.001 | 0.029 | 0.165 | 0 | 0 |
| high | 109 | 109 | 0 | 101281 | 0.170 | 0.514 | 15.628 | 0.085 | 0.170 | 1.783 | 59833561 | 0 |
| medium | 7890 | 7888 | 0 | 290498 | 3.716 | 14.026 | 730.158 | 4.117 | 6.074 | 3.060 | 51216879894 | 0 |
| tiny | 153 | 0 | 152 | 0 | censored | censored | 19962327.532 | 110380.801 | censored | censored | 11107309 | 0 |
| very-tiny | 1847 | 0 | 1846 | 0 | censored | censored | 16415371799.757 | 93616158.385 | censored | censored | 12501175 | 0 |

Fixed absolute probe: hash fraction 0.00010000, m=20, payouts 88, mean/P95 interval 2.047/4.745 days, theoretical threshold 1.149 days, solo 277.778 days, unpaid 0 raw units.

### 10000 miners, giant70, Strict, N=187, share interval=1s

Run 146.934 days; history 0 wins / 0 raw units; blocked coverage 3280, blocked clipped minimum 0; permanently held raw units 0 (includes historical custody assumption); outstanding fundable roots 0; first split opportunity Some(1).
Signed check: won 8459045090241 = gross paid 8459045090241 + unpaid 33888852548 - advanced 33888852548 + unallocated 0; reserved 0 is included in unpaid; reimbursement 100466000000 is included in gross paid. Outside claims 15867 emitted 77167723630599; ending vault 2002373231279160. Peak advanced 33918104594 raw units (6.897 mean rewards); advanced held by departed miners 191682274; paid beyond a miner's own bound 10789380701.

| Period | Opportunities | Won | Splits | Payouts/opportunity | Payouts/won | Eligible rank>N mean/max | Fee/split sats | Gross front/split sats | Net external/split sats |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| first 100 | 100 | 100 | 99 | 57.120 | 57.120 | 0.00/0 | 8792.7 | 50144.1 | 49444.1 |
| later | 4900 | 1620 | 1621 | 19.338 | 58.490 | 0.00/2 | 8891.5 | 50776.6 | 50076.6 |

Waits pool completed payout intervals within class (event weighted); theory and solo columns average miners (miner weighted), so their ratio is not an unbiased speedup. Unpaid miners and zero-share miners are shown explicitly. Theoretical waits average each miner's m*C divided by its absolute expected raw-token earning rate. First payout waits start at study entry or actual joining. Reimbursement includes BCH dust and fee, rather than BCH miner fee alone.

| Hash class | Miners | Paid miners | Zero shares | Intervals | Mean wait d | P95 wait d | Mean solo d | Mean m*C/rate d | First payout mean d | Token reimbursement % | Unpaid raw units | Reserved raw units |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| giant | 1 | 1 | 0 | 1719 | 0.069 | 0.188 | 0.040 | 0.000 | 0.062 | 0.028 | 0 | 0 |
| medium | 2195 | 2048 | 0 | 95299 | 1.785 | 6.614 | 691.375 | 3.961 | 4.141 | 4.287 | 16639736702 | 0 |
| small | 2520 | 649 | 148 | 750 | 22.209 | 55.093 | 58740.636 | 328.456 | 31.549 | 5.702 | 16831620612 | 0 |
| tiny | 2460 | 0 | 2019 | 0 | censored | censored | 5896173.020 | 33426.999 | censored | censored | 412262982 | 0 |
| very-tiny | 2824 | 0 | 2815 | 0 | censored | censored | 947397972.751 | 5206551.065 | censored | censored | 5232252 | 0 |

Fixed absolute probe: hash fraction 0.00010000, m=20, payouts 41, mean/P95 interval 2.400/5.778 days, theoretical threshold 1.142 days, solo 277.778 days, unpaid 5147849 raw units.

### 10000 miners, log-uniform, Strict, N=187, share interval=1s

Run 161.422 days; history 0 wins / 0 raw units; blocked coverage 4094, blocked clipped minimum 0; permanently held raw units 0 (includes historical custody assumption); outstanding fundable roots 0; first split opportunity Some(1).
Signed check: won 4452581819746 = gross paid 4452581819746 + unpaid 31102977998 - advanced 31102977998 + unallocated 0; reserved 0 is included in unpaid; reimbursement 152751000000 is included in gross paid. Outside claims 17410 emitted 84646698832325; ending vault 1998900719347929. Peak advanced 31121261829 raw units (6.332 mean rewards); advanced held by departed miners 1908711271; paid beyond a miner's own bound 4032520473.

| Period | Opportunities | Won | Splits | Payouts/opportunity | Payouts/won | Eligible rank>N mean/max | Fee/split sats | Gross front/split sats | Net external/split sats |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| first 100 | 100 | 100 | 99 | 141.620 | 141.620 | 0.00/0 | 19973.6 | 121499.2 | 120799.2 |
| later | 4900 | 806 | 807 | 28.283 | 171.947 | 0.87/19 | 23731.1 | 145478.3 | 144778.3 |

Waits pool completed payout intervals within class (event weighted); theory and solo columns average miners (miner weighted), so their ratio is not an unbiased speedup. Unpaid miners and zero-share miners are shown explicitly. Theoretical waits average each miner's m*C divided by its absolute expected raw-token earning rate. First payout waits start at study entry or actual joining. Reimbursement includes BCH dust and fee, rather than BCH miner fee alone.

| Hash class | Miners | Paid miners | Zero shares | Intervals | Mean wait d | P95 wait d | Mean solo d | Mean m*C/rate d | First payout mean d | Token reimbursement % | Unpaid raw units | Reserved raw units |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| high | 318 | 318 | 0 | 60732 | 0.593 | 1.416 | 20.784 | 0.118 | 0.198 | 3.071 | 29878720 | 0 |
| medium | 2520 | 2357 | 0 | 88704 | 2.409 | 9.171 | 587.498 | 3.372 | 5.591 | 3.710 | 16723801395 | 0 |
| small | 2460 | 409 | 108 | 231 | 44.087 | 81.373 | 56170.578 | 319.759 | 49.502 | 6.249 | 14104377757 | 0 |
| tiny | 2568 | 0 | 2062 | 0 | censored | censored | 6109279.423 | 34011.849 | censored | censored | 241610480 | 0 |
| very-tiny | 2134 | 0 | 2123 | 0 | censored | censored | 370672804.550 | 2037174.030 | censored | censored | 3309646 | 0 |

Fixed absolute probe: hash fraction 0.00010000, m=20, payouts 24, mean/P95 interval 4.651/8.732 days, theoretical threshold 1.143 days, solo 277.778 days, unpaid 4100442 raw units.

### 10000 miners, heavy-tail, Strict, N=187, share interval=1s

Run 180.239 days; history 0 wins / 0 raw units; blocked coverage 4984, blocked clipped minimum 0; permanently held raw units 0 (includes historical custody assumption); outstanding fundable roots 0; first split opportunity Some(1).
Signed check: won 78984245161 = gross paid 78984245161 + unpaid 26844747449 - advanced 26844747449 + unallocated 0; reserved 0 is included in unpaid; reimbursement 2050000000 is included in gross paid. Outside claims 19354 emitted 94030267160904; ending vault 1993890748593935. Peak advanced 26844747449 raw units (5.438 mean rewards); advanced held by departed miners 1646799540; paid beyond a miner's own bound 93387672.

| Period | Opportunities | Won | Splits | Payouts/opportunity | Payouts/won | Eligible rank>N mean/max | Fee/split sats | Gross front/split sats | Net external/split sats |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| first 100 | 100 | 11 | 11 | 13.870 | 126.091 | 11.68/26 | 17751.9 | 107321.0 | 106621.0 |
| later | 4900 | 5 | 5 | 0.135 | 132.600 | 6.96/9 | 18604.6 | 112762.6 | 112062.6 |

Waits pool completed payout intervals within class (event weighted); theory and solo columns average miners (miner weighted), so their ratio is not an unbiased speedup. Unpaid miners and zero-share miners are shown explicitly. Theoretical waits average each miner's m*C divided by its absolute expected raw-token earning rate. First payout waits start at study entry or actual joining. Reimbursement includes BCH dust and fee, rather than BCH miner fee alone.

| Hash class | Miners | Paid miners | Zero shares | Intervals | Mean wait d | P95 wait d | Mean solo d | Mean m*C/rate d | First payout mean d | Token reimbursement % | Unpaid raw units | Reserved raw units |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| giant | 1 | 1 | 0 | 1 | 0.266 | 0.266 | 0.241 | 0.001 | 0.115 | 0.040 | 0 | 0 |
| high | 109 | 108 | 0 | 617 | 11.047 | 59.724 | 15.628 | 0.084 | 0.161 | 1.747 | 114156823 | 0 |
| medium | 7890 | 954 | 0 | 369 | 34.782 | 127.314 | 730.158 | 4.101 | 22.103 | 4.078 | 26730477318 | 0 |
| tiny | 153 | 0 | 148 | 0 | censored | censored | 19962327.532 | 109927.756 | censored | censored | 0 | 0 |
| very-tiny | 1847 | 0 | 1841 | 0 | censored | censored | 16415371799.757 | 93231921.833 | censored | censored | 113308 | 0 |

Fixed absolute probe: hash fraction 0.00010000, m=20, payouts 1, mean/P95 interval censored/censored days, theoretical threshold 1.144 days, solo 277.778 days, unpaid 0 raw units.

### 10000 miners, giant70, Strict, N=187, share interval=10s

Run 145.565 days; history 0 wins / 0 raw units; blocked coverage 0, blocked clipped minimum 0; permanently held raw units 0 (includes historical custody assumption); outstanding fundable roots 0; first split opportunity Some(1).
Signed check: won 24254093199580 = gross paid 24254093199580 + unpaid 52642808306 - advanced 52642808306 + unallocated 0; reserved 0 is included in unpaid; reimbursement 318177000000 is included in gross paid. Outside claims 15714 emitted 76225378928255; ending vault 1987520527872165. Peak advanced 53832561183 raw units (11.098 mean rewards); advanced held by departed miners 0; paid beyond a miner's own bound 0.

| Period | Opportunities | Won | Splits | Payouts/opportunity | Payouts/won | Eligible rank>N mean/max | Fee/split sats | Gross front/split sats | Net external/split sats |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| first 100 | 100 | 100 | 99 | 57.780 | 57.780 | 0.00/0 | 8879.9 | 50701.3 | 50001.3 |
| later | 4900 | 4900 | 4901 | 63.755 | 63.755 | 0.00/0 | 9584.2 | 55197.2 | 54497.2 |

Waits pool completed payout intervals within class (event weighted); theory and solo columns average miners (miner weighted), so their ratio is not an unbiased speedup. Unpaid miners and zero-share miners are shown explicitly. Theoretical waits average each miner's m*C divided by its absolute expected raw-token earning rate. First payout waits start at study entry or actual joining. Reimbursement includes BCH dust and fee, rather than BCH miner fee alone.

| Hash class | Miners | Paid miners | Zero shares | Intervals | Mean wait d | P95 wait d | Mean solo d | Mean m*C/rate d | First payout mean d | Token reimbursement % | Unpaid raw units | Reserved raw units |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| giant | 1 | 1 | 0 | 4999 | 0.029 | 0.087 | 0.040 | 0.000 | 0.056 | 0.028 | 0 | 0 |
| medium | 2195 | 2189 | 0 | 305826 | 0.837 | 2.832 | 691.375 | 3.971 | 5.961 | 4.762 | 31096272001 | 0 |
| small | 2520 | 1118 | 1022 | 4013 | 14.166 | 58.359 | 58740.636 | 329.298 | 49.309 | 5.383 | 20789059556 | 0 |
| tiny | 2460 | 22 | 2405 | 9 | 0.932 | 1.181 | 5896173.020 | 33512.616 | 86.052 | 8.730 | 742632846 | 0 |
| very-tiny | 2824 | 0 | 2823 | 0 | censored | censored | 947397972.751 | 5219886.651 | censored | censored | 14843903 | 0 |

Fixed absolute probe: hash fraction 0.00010000, m=20, payouts 117, mean/P95 interval 1.216/3.040 days, theoretical threshold 1.145 days, solo 277.778 days, unpaid 10683075 raw units.

### 10000 miners, log-uniform, Strict, N=187, share interval=10s

Run 162.130 days; history 0 wins / 0 raw units; blocked coverage 0, blocked clipped minimum 0; permanently held raw units 0 (includes historical custody assumption); outstanding fundable roots 0; first split opportunity Some(1).
Signed check: won 24201087401856 = gross paid 24201087401856 + unpaid 57759770934 - advanced 57759770934 + unallocated 0; reserved 0 is included in unpaid; reimbursement 645357000000 is included in gross paid. Outside claims 17634 emitted 85343403990422; ending vault 1978455508607722. Peak advanced 58498680863 raw units (12.086 mean rewards); advanced held by departed miners 0; paid beyond a miner's own bound 11720108899.

| Period | Opportunities | Won | Splits | Payouts/opportunity | Payouts/won | Eligible rank>N mean/max | Fee/split sats | Gross front/split sats | Net external/split sats |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| first 100 | 100 | 100 | 99 | 162.400 | 162.400 | 0.96/24 | 22723.3 | 139046.8 | 138346.8 |
| later | 4900 | 4900 | 4901 | 128.391 | 128.391 | 0.00/0 | 18049.8 | 109222.2 | 108522.2 |

Waits pool completed payout intervals within class (event weighted); theory and solo columns average miners (miner weighted), so their ratio is not an unbiased speedup. Unpaid miners and zero-share miners are shown explicitly. Theoretical waits average each miner's m*C divided by its absolute expected raw-token earning rate. First payout waits start at study entry or actual joining. Reimbursement includes BCH dust and fee, rather than BCH miner fee alone.

| Hash class | Miners | Paid miners | Zero shares | Intervals | Mean wait d | P95 wait d | Mean solo d | Mean m*C/rate d | First payout mean d | Token reimbursement % | Unpaid raw units | Reserved raw units |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| high | 318 | 318 | 0 | 93070 | 0.484 | 1.452 | 20.784 | 0.118 | 0.272 | 0.868 | 639881228 | 0 |
| medium | 2520 | 2520 | 0 | 543724 | 0.609 | 1.878 | 587.498 | 3.388 | 5.077 | 4.097 | 34287898507 | 0 |
| small | 2460 | 1218 | 892 | 4466 | 15.526 | 58.895 | 56170.578 | 321.237 | 52.637 | 5.199 | 21999014436 | 0 |
| tiny | 2568 | 33 | 2503 | 6 | 1.085 | 1.374 | 6109279.423 | 34169.111 | 78.269 | 7.281 | 798718108 | 0 |
| very-tiny | 2134 | 1 | 2131 | 1 | 1.247 | 1.247 | 370672804.550 | 2046593.388 | 94.613 | 9.962 | 34258655 | 0 |

Fixed absolute probe: hash fraction 0.00010000, m=20, payouts 163, mean/P95 interval 0.984/2.180 days, theoretical threshold 1.148 days, solo 277.778 days, unpaid 17176836 raw units.

### 10000 miners, heavy-tail, Strict, N=187, share interval=10s

Run 180.020 days; history 0 wins / 0 raw units; blocked coverage 0, blocked clipped minimum 0; permanently held raw units 0 (includes historical custody assumption); outstanding fundable roots 0; first split opportunity Some(1).
Signed check: won 24132617566369 = gross paid 24132617566369 + unpaid 94444770019 - advanced 94444770019 + unallocated 0; reserved 0 is included in unpaid; reimbursement 597999000000 is included in gross paid. Outside claims 19450 emitted 93947825434660; ending vault 1969919556998971. Peak advanced 95984473154 raw units (19.887 mean rewards); advanced held by departed miners 187946131; paid beyond a miner's own bound 38094718571.

| Period | Opportunities | Won | Splits | Payouts/opportunity | Payouts/won | Eligible rank>N mean/max | Fee/split sats | Gross front/split sats | Net external/split sats |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| first 100 | 100 | 100 | 99 | 144.450 | 144.450 | 4.25/73 | 20348.1 | 123889.0 | 123189.0 |
| later | 4900 | 4900 | 4901 | 119.093 | 119.093 | 0.00/0 | 16832.0 | 101450.1 | 100750.1 |

Waits pool completed payout intervals within class (event weighted); theory and solo columns average miners (miner weighted), so their ratio is not an unbiased speedup. Unpaid miners and zero-share miners are shown explicitly. Theoretical waits average each miner's m*C divided by its absolute expected raw-token earning rate. First payout waits start at study entry or actual joining. Reimbursement includes BCH dust and fee, rather than BCH miner fee alone.

| Hash class | Miners | Paid miners | Zero shares | Intervals | Mean wait d | P95 wait d | Mean solo d | Mean m*C/rate d | First payout mean d | Token reimbursement % | Unpaid raw units | Reserved raw units |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| giant | 1 | 1 | 0 | 1666 | 0.031 | 0.094 | 0.241 | 0.001 | 0.010 | 0.166 | 0 | 0 |
| high | 109 | 109 | 0 | 97615 | 0.176 | 0.592 | 15.628 | 0.085 | 0.201 | 1.142 | 227712418 | 0 |
| medium | 7890 | 7889 | 0 | 490719 | 2.232 | 8.480 | 730.158 | 4.125 | 6.428 | 3.421 | 94179740663 | 0 |
| tiny | 153 | 0 | 152 | 0 | censored | censored | 19962327.532 | 110588.531 | censored | censored | 17169097 | 0 |
| very-tiny | 1847 | 0 | 1846 | 0 | censored | censored | 16415371799.757 | 93792338.098 | censored | censored | 20147841 | 0 |

Fixed absolute probe: hash fraction 0.00010000, m=20, payouts 144, mean/P95 interval 1.241/3.054 days, theoretical threshold 1.151 days, solo 277.778 days, unpaid 18458510 raw units.

