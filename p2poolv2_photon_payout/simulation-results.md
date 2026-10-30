# Seeded minimum-payout study

Seed `0x70686f746f6e6d70`, SplitMix64; 5,000 potential pool wins per full run, 100 prior synthetic wins held under an UNSPECIFIED bootstrap custody assumption. The strict genesis probe has no such history. These are scenarios, not owner parameter choices or live PHOTON rates.

Full pool hash is one quarter of network work: initial full-pool win mean 2,400 seconds; fixed outside-work win mean 800 seconds. Churn changes pool share/win arrival rates without renormalizing individual hash. Shares are equal work; the newest 4,320 are the PPLNS window with the historical 4,096 distinct-script stop. Wins are a subset of chain-share events; their own share is appended after parent-window credit. No uncles, adversarial network, forks or reorgs are modeled. 10 percent initially absent join at two-thirds; another 10 percent leave at one-third. Leaving does not erase prior shares or debt.

Every accepted pool or outside claim uses conserved reward floor(vault/420000)-1-tail, tail uniform 0..65535, vault starts at 2088000000000000 raw units. C=1000000 raw units, multiples uniform in 10/20/30/50, N=64 unless labeled. No token price or oracle. Reimbursement is C per actual payee. At every 97th opportunity, a valid split waits ten more opportunities for funding, preserving its immutable list and reservations. Permanently blocked roots are never refreshed or spent; their tokens are inaccessible. Their face value is backing for accounting only, and does not establish settlement liveness.

All BCH costs are exact lean-builder serialization with each simulated list, debt, multiplier, and actual amount: 35-byte P2SH32 payees, P2PKH submitter/change, one 100-byte synthetic P2PKH funding unlock and 700 inherited satoshis. Fee equals bytes at 1 sat/byte. Gross fronting is fee plus all token-output BCH minima; net external subtracts inherited root BCH. Funding placeholder length equals a real fixture signature but no simulated transaction is signed or individually VM-verified. Default dust is the builder's conservative largest-token-prefix minimum. Cost is charged only for successful funded splits.

Classes by fraction of original full pool hash: giant >=0.1; high [0.001,0.1); medium [1e-5,0.001); small [1e-7,1e-5); tiny [1e-9,1e-7); very-tiny <1e-9. A separate m=20 miner has fixed absolute fraction 0.0001 at both population sizes. Population comparisons without that probe change individual earning rates and cannot test population independence.

### 1000 miners, giant70, Strict, N=64, share interval=10s

Run 2.859 days; history 0 wins / 0 raw units; blocked coverage 100, blocked clipped minimum 0; permanently held raw units 0 (includes historical custody assumption); outstanding fundable roots 0; first split opportunity None.
Integer check: won 0 = gross paid 0 + unpaid 0 + unallocated 0; reserved 0 is included in unpaid; reimbursement 0 is included in gross paid. Outside claims 304 emitted 1510759412508; ending vault 2086489240587492.

| Period | Opportunities | Won | Splits | Payouts/opportunity | Payouts/won | Eligible rank>N mean/max | Fee/split sats | Gross front/split sats | Net external/split sats |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| first 100 | 100 | 0 | 0 | 0.000 | 0.000 | 0.00/0 | 0.0 | 0.0 | 0.0 |
| later | 0 | 0 | 0 | 0.000 | 0.000 | 0.00/0 | 0.0 | 0.0 | 0.0 |

Waits pool completed payout intervals within class (event weighted); theory and solo columns average miners (miner weighted), so their ratio is not an unbiased speedup. Unpaid miners and zero-share miners are shown explicitly. Theoretical waits average each miner's m*C divided by its absolute expected raw-token earning rate. First payout waits start at study entry or actual joining. Reimbursement includes BCH dust and fee, rather than BCH miner fee alone.

| Hash class | Miners | Paid miners | Zero shares | Intervals | Mean wait d | P95 wait d | Mean solo d | Mean m*C/rate d | First payout mean d | Token reimbursement % | Unpaid raw units | Reserved raw units |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| giant | 1 | 0 | 0 | 0 | censored | censored | 0.040 | 0.000 | censored | censored | 0 | 0 |
| high | 94 | 0 | 0 | 0 | censored | censored | 13.363 | 0.075 | censored | censored | 0 | 0 |
| medium | 257 | 0 | 76 | 0 | censored | censored | 602.303 | 3.690 | censored | censored | 0 | 0 |
| small | 246 | 0 | 236 | 0 | censored | censored | 57803.696 | 327.752 | censored | censored | 0 | 0 |
| tiny | 242 | 0 | 242 | 0 | censored | censored | 6265193.915 | 32522.317 | censored | censored | 0 | 0 |
| very-tiny | 160 | 0 | 160 | 0 | censored | censored | 142787836.410 | 877266.625 | censored | censored | 0 | 0 |

Fixed absolute probe: hash fraction 0.00010000, m=20, payouts 0, mean/P95 interval censored/censored days, theoretical threshold 1.118 days, solo 277.778 days, unpaid 0 raw units.

### 1000 miners, giant70, HoldBlockedRoots, N=64, share interval=10s

Run 145.663 days; history 100 wins / 496918064905 raw units; blocked coverage 0, blocked clipped minimum 33; permanently held raw units 655774293362 (includes historical custody assumption); outstanding fundable roots 0; first split opportunity Some(1).
Integer check: won 24728690269411 = gross paid 24072915976049 + unpaid 655774293362 + unallocated 0; reserved 0 is included in unpaid; reimbursement 5038000000 is included in gross paid. Outside claims 16046 emitted 77794026247171; ending vault 1985477283483418.

| Period | Opportunities | Won | Splits | Payouts/opportunity | Payouts/won | Eligible rank>N mean/max | Fee/split sats | Gross front/split sats | Net external/split sats |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| first 100 | 100 | 100 | 99 | 0.990 | 0.990 | 184.85/204 | 1436.0 | 2816.0 | 2116.0 |
| later | 4900 | 4900 | 4868 | 1.008 | 1.008 | 334.42/405 | 1437.9 | 2828.2 | 2128.2 |

Waits pool completed payout intervals within class (event weighted); theory and solo columns average miners (miner weighted), so their ratio is not an unbiased speedup. Unpaid miners and zero-share miners are shown explicitly. Theoretical waits average each miner's m*C divided by its absolute expected raw-token earning rate. First payout waits start at study entry or actual joining. Reimbursement includes BCH dust and fee, rather than BCH miner fee alone.

| Hash class | Miners | Paid miners | Zero shares | Intervals | Mean wait d | P95 wait d | Mean solo d | Mean m*C/rate d | First payout mean d | Token reimbursement % | Unpaid raw units | Reserved raw units |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| giant | 1 | 1 | 0 | 3712 | 0.039 | 0.116 | 0.040 | 0.000 | 0.019 | 0.021 | 6437967634 | 0 |
| high | 94 | 94 | 0 | 1055 | 9.191 | 21.431 | 13.363 | 0.077 | 14.585 | 0.021 | 230895713031 | 0 |
| medium | 257 | 79 | 0 | 97 | 29.891 | 67.868 | 602.303 | 3.784 | 72.590 | 0.024 | 405130751410 | 0 |
| small | 246 | 0 | 94 | 0 | censored | censored | 57803.696 | 336.098 | censored | censored | 13183696928 | 0 |
| tiny | 242 | 0 | 236 | 0 | censored | censored | 6265193.915 | 33350.560 | censored | censored | 126164359 | 0 |
| very-tiny | 160 | 0 | 160 | 0 | censored | censored | 142787836.410 | 899607.878 | censored | censored | 0 | 0 |

Fixed absolute probe: hash fraction 0.00010000, m=20, payouts 0, mean/P95 interval censored/censored days, theoretical threshold 1.146 days, solo 277.778 days, unpaid 2616951161 raw units.

### 1000 miners, log-uniform, HoldBlockedRoots, N=64, share interval=10s

Run 163.389 days; history 100 wins / 496912687558 raw units; blocked coverage 0, blocked clipped minimum 3; permanently held raw units 511756241342 (includes historical custody assumption); outstanding fundable roots 0; first split opportunity Some(1).
Integer check: won 24670909011693 = gross paid 24159152770351 + unpaid 511756241342 + unallocated 0; reserved 0 is included in unpaid; reimbursement 9529000000 is included in gross paid. Outside claims 18101 emitted 87542679601071; ending vault 1975786411387236.

| Period | Opportunities | Won | Splits | Payouts/opportunity | Payouts/won | Eligible rank>N mean/max | Fee/split sats | Gross front/split sats | Net external/split sats |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| first 100 | 100 | 100 | 99 | 0.990 | 0.990 | 246.47/267 | 1436.0 | 2816.0 | 2116.0 |
| later | 4900 | 4900 | 4898 | 1.924 | 1.924 | 397.56/465 | 1553.5 | 3585.9 | 2885.9 |

Waits pool completed payout intervals within class (event weighted); theory and solo columns average miners (miner weighted), so their ratio is not an unbiased speedup. Unpaid miners and zero-share miners are shown explicitly. Theoretical waits average each miner's m*C divided by its absolute expected raw-token earning rate. First payout waits start at study entry or actual joining. Reimbursement includes BCH dust and fee, rather than BCH miner fee alone.

| Hash class | Miners | Paid miners | Zero shares | Intervals | Mean wait d | P95 wait d | Mean solo d | Mean m*C/rate d | First payout mean d | Token reimbursement % | Unpaid raw units | Reserved raw units |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| high | 150 | 150 | 0 | 8834 | 2.272 | 6.710 | 8.575 | 0.050 | 6.167 | 0.039 | 209594443391 | 0 |
| medium | 274 | 135 | 0 | 410 | 21.447 | 47.171 | 587.205 | 3.418 | 63.320 | 0.046 | 290218239029 | 0 |
| small | 233 | 0 | 87 | 0 | censored | censored | 55337.249 | 336.952 | censored | censored | 11841165226 | 0 |
| tiny | 256 | 0 | 251 | 0 | censored | censored | 6026253.083 | 37276.069 | censored | censored | 102393696 | 0 |
| very-tiny | 87 | 0 | 87 | 0 | censored | censored | 66155785.076 | 414989.646 | censored | censored | 0 | 0 |

Fixed absolute probe: hash fraction 0.00010000, m=20, payouts 1, mean/P95 interval censored/censored days, theoretical threshold 1.149 days, solo 277.778 days, unpaid 1378070570 raw units.

### 1000 miners, heavy-tail, HoldBlockedRoots, N=64, share interval=10s

Run 164.022 days; history 100 wins / 496903598216 raw units; blocked coverage 0, blocked clipped minimum 38; permanently held raw units 680705337624 (includes historical custody assumption); outstanding fundable roots 0; first split opportunity Some(1).
Integer check: won 24666369048515 = gross paid 23985663710891 + unpaid 680705337624 + unallocated 0; reserved 0 is included in unpaid; reimbursement 16570000000 is included in gross paid. Outside claims 18169 emitted 87867081023597; ending vault 1975466549927888.

| Period | Opportunities | Won | Splits | Payouts/opportunity | Payouts/won | Eligible rank>N mean/max | Fee/split sats | Gross front/split sats | Net external/split sats |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| first 100 | 100 | 100 | 97 | 1.100 | 1.100 | 634.69/636 | 1453.4 | 2927.9 | 2227.9 |
| later | 4900 | 4900 | 4865 | 3.359 | 3.359 | 662.23/735 | 1744.2 | 4804.5 | 4104.5 |

Waits pool completed payout intervals within class (event weighted); theory and solo columns average miners (miner weighted), so their ratio is not an unbiased speedup. Unpaid miners and zero-share miners are shown explicitly. Theoretical waits average each miner's m*C divided by its absolute expected raw-token earning rate. First payout waits start at study entry or actual joining. Reimbursement includes BCH dust and fee, rather than BCH miner fee alone.

| Hash class | Miners | Paid miners | Zero shares | Intervals | Mean wait d | P95 wait d | Mean solo d | Mean m*C/rate d | First payout mean d | Token reimbursement % | Unpaid raw units | Reserved raw units |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| giant | 1 | 1 | 0 | 2425 | 0.068 | 0.161 | 0.172 | 0.001 | 0.018 | 0.053 | 831816527 | 0 |
| high | 143 | 143 | 0 | 9035 | 2.012 | 6.858 | 14.896 | 0.081 | 4.634 | 0.069 | 123687610673 | 0 |
| medium | 656 | 656 | 0 | 4310 | 16.086 | 36.005 | 77.410 | 0.463 | 18.692 | 0.081 | 556185910424 | 0 |
| tiny | 70 | 0 | 70 | 0 | censored | censored | 9146399.273 | 63241.822 | censored | censored | 0 | 0 |
| very-tiny | 130 | 0 | 130 | 0 | censored | censored | 2232137981.665 | 12985281.187 | censored | censored | 0 | 0 |

Fixed absolute probe: hash fraction 0.00010000, m=20, payouts 2, mean/P95 interval 76.940/76.940 days, theoretical threshold 1.149 days, solo 277.778 days, unpaid 542243409 raw units.

### 10000 miners, giant70, HoldBlockedRoots, N=64, share interval=10s

Run 145.484 days; history 100 wins / 496888572957 raw units; blocked coverage 0, blocked clipped minimum 33; permanently held raw units 655688168374 (includes historical custody assumption); outstanding fundable roots 0; first split opportunity Some(1).
Integer check: won 24727253991935 = gross paid 24071565823561 + unpaid 655688168374 + unallocated 0; reserved 0 is included in unpaid; reimbursement 18385000000 is included in gross paid. Outside claims 16062 emitted 77871097217568; ending vault 1985401648790497.

| Period | Opportunities | Won | Splits | Payouts/opportunity | Payouts/won | Eligible rank>N mean/max | Fee/split sats | Gross front/split sats | Net external/split sats |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| first 100 | 100 | 100 | 99 | 0.990 | 0.990 | 1335.17/1478 | 1436.0 | 2816.0 | 2116.0 |
| later | 4900 | 4900 | 4868 | 3.732 | 3.732 | 2615.27/3194 | 1793.3 | 5116.5 | 4416.5 |

Waits pool completed payout intervals within class (event weighted); theory and solo columns average miners (miner weighted), so their ratio is not an unbiased speedup. Unpaid miners and zero-share miners are shown explicitly. Theoretical waits average each miner's m*C divided by its absolute expected raw-token earning rate. First payout waits start at study entry or actual joining. Reimbursement includes BCH dust and fee, rather than BCH miner fee alone.

| Hash class | Miners | Paid miners | Zero shares | Intervals | Mean wait d | P95 wait d | Mean solo d | Mean m*C/rate d | First payout mean d | Token reimbursement % | Unpaid raw units | Reserved raw units |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| giant | 1 | 1 | 0 | 4966 | 0.029 | 0.087 | 0.040 | 0.000 | 0.012 | 0.028 | 3406393320 | 0 |
| medium | 2195 | 1737 | 0 | 11681 | 11.408 | 30.331 | 691.375 | 3.975 | 38.520 | 0.220 | 532481931389 | 0 |
| small | 2520 | 0 | 1018 | 0 | censored | censored | 58740.636 | 329.620 | censored | censored | 118690593356 | 0 |
| tiny | 2460 | 0 | 2406 | 0 | censored | censored | 5896173.020 | 33545.433 | censored | censored | 1093562644 | 0 |
| very-tiny | 2824 | 0 | 2823 | 0 | censored | censored | 947397972.751 | 5224998.268 | censored | censored | 15687665 | 0 |

Fixed absolute probe: hash fraction 0.00010000, m=20, payouts 5, mean/P95 interval 25.265/27.892 days, theoretical threshold 1.146 days, solo 277.778 days, unpaid 224818407 raw units.

### 10000 miners, log-uniform, HoldBlockedRoots, N=64, share interval=10s

Run 162.169 days; history 100 wins / 496881895476 raw units; blocked coverage 0, blocked clipped minimum 102; permanently held raw units 993908573658 (includes historical custody assumption); outstanding fundable roots 0; first split opportunity Some(1).
Integer check: won 24671363628228 = gross paid 23677455054570 + unpaid 993908573658 + unallocated 0; reserved 0 is included in unpaid; reimbursement 38980000000 is included in gross paid. Outside claims 18013 emitted 87128209161273; ending vault 1976200427210499.

| Period | Opportunities | Won | Splits | Payouts/opportunity | Payouts/won | Eligible rank>N mean/max | Fee/split sats | Gross front/split sats | Net external/split sats |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| first 100 | 100 | 100 | 98 | 6.070 | 6.070 | 1903.71/2051 | 2112.4 | 7154.1 | 6454.1 |
| later | 4900 | 4900 | 4800 | 7.831 | 7.831 | 3280.13/3902 | 2348.3 | 8659.3 | 7959.3 |

Waits pool completed payout intervals within class (event weighted); theory and solo columns average miners (miner weighted), so their ratio is not an unbiased speedup. Unpaid miners and zero-share miners are shown explicitly. Theoretical waits average each miner's m*C divided by its absolute expected raw-token earning rate. First payout waits start at study entry or actual joining. Reimbursement includes BCH dust and fee, rather than BCH miner fee alone.

| Hash class | Miners | Paid miners | Zero shares | Intervals | Mean wait d | P95 wait d | Mean solo d | Mean m*C/rate d | First payout mean d | Token reimbursement % | Unpaid raw units | Reserved raw units |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| high | 318 | 318 | 0 | 17308 | 2.572 | 3.877 | 20.784 | 0.119 | 1.267 | 0.163 | 93425761511 | 0 |
| medium | 2520 | 1994 | 0 | 19360 | 9.671 | 28.634 | 587.498 | 3.392 | 35.603 | 0.166 | 765150345026 | 0 |
| small | 2460 | 0 | 894 | 0 | censored | censored | 56170.578 | 321.591 | censored | censored | 133826603526 | 0 |
| tiny | 2568 | 0 | 2501 | 0 | censored | censored | 6109279.423 | 34206.717 | censored | censored | 1451238781 | 0 |
| very-tiny | 2134 | 0 | 2131 | 0 | censored | censored | 370672804.550 | 2048845.804 | censored | censored | 54624814 | 0 |

Fixed absolute probe: hash fraction 0.00010000, m=20, payouts 5, mean/P95 interval 29.919/32.954 days, theoretical threshold 1.149 days, solo 277.778 days, unpaid 497551214 raw units.

### 10000 miners, heavy-tail, HoldBlockedRoots, N=64, share interval=10s

Run 179.739 days; history 100 wins / 496859937136 raw units; blocked coverage 0, blocked clipped minimum 235; permanently held raw units 1643739603498 (includes historical custody assumption); outstanding fundable roots 0; first split opportunity Some(1).
Integer check: won 24602905906155 = gross paid 22959166302657 + unpaid 1643739603498 + unallocated 0; reserved 0 is included in unpaid; reimbursement 64331000000 is included in gross paid. Outside claims 19821 emitted 95685408782594; ending vault 1967711685311251.

| Period | Opportunities | Won | Splits | Payouts/opportunity | Payouts/won | Eligible rank>N mean/max | Fee/split sats | Gross front/split sats | Net external/split sats |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| first 100 | 100 | 100 | 95 | 5.830 | 5.830 | 4266.46/4922 | 2105.4 | 7106.9 | 6406.9 |
| later | 4900 | 4900 | 4670 | 13.010 | 13.010 | 6718.88/7494 | 3089.2 | 13387.8 | 12687.8 |

Waits pool completed payout intervals within class (event weighted); theory and solo columns average miners (miner weighted), so their ratio is not an unbiased speedup. Unpaid miners and zero-share miners are shown explicitly. Theoretical waits average each miner's m*C divided by its absolute expected raw-token earning rate. First payout waits start at study entry or actual joining. Reimbursement includes BCH dust and fee, rather than BCH miner fee alone.

| Hash class | Miners | Paid miners | Zero shares | Intervals | Mean wait d | P95 wait d | Mean solo d | Mean m*C/rate d | First payout mean d | Token reimbursement % | Unpaid raw units | Reserved raw units |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| giant | 1 | 1 | 0 | 1641 | 0.031 | 0.095 | 0.241 | 0.001 | 0.204 | 0.166 | 562848606 | 0 |
| high | 109 | 109 | 0 | 22762 | 0.746 | 2.010 | 15.628 | 0.085 | 1.406 | 0.264 | 22174604102 | 0 |
| medium | 7890 | 6998 | 0 | 32820 | 18.954 | 68.729 | 730.158 | 4.130 | 42.830 | 0.299 | 1620969521514 | 0 |
| tiny | 153 | 0 | 152 | 0 | censored | censored | 19962327.532 | 110710.430 | censored | censored | 12190925 | 0 |
| very-tiny | 1847 | 0 | 1846 | 0 | censored | censored | 16415371799.757 | 93895723.458 | censored | censored | 20438351 | 0 |

Fixed absolute probe: hash fraction 0.00010000, m=20, payouts 11, mean/P95 interval 15.272/21.830 days, theoretical threshold 1.152 days, solo 277.778 days, unpaid 368679123 raw units.

### 10000 miners, giant70, Strict, N=64, share interval=10s

Run 145.484 days; history 100 wins / 496888572957 raw units; blocked coverage 0, blocked clipped minimum 3561; permanently held raw units 496888572957 (includes historical custody assumption); outstanding fundable roots 0; first split opportunity Some(1).
Integer check: won 7512784469887 = gross paid 7015895896930 + unpaid 496888572957 + unallocated 0; reserved 0 is included in unpaid; reimbursement 4429000000 is included in gross paid. Outside claims 16062 emitted 78180729184409; ending vault 2002306486345704.

| Period | Opportunities | Won | Splits | Payouts/opportunity | Payouts/won | Eligible rank>N mean/max | Fee/split sats | Gross front/split sats | Net external/split sats |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| first 100 | 100 | 100 | 99 | 0.990 | 0.990 | 1335.17/1478 | 1436.0 | 2816.0 | 2116.0 |
| later | 4900 | 1339 | 1340 | 0.884 | 3.234 | 2191.03/2589 | 1725.1 | 4678.2 | 3978.2 |

Waits pool completed payout intervals within class (event weighted); theory and solo columns average miners (miner weighted), so their ratio is not an unbiased speedup. Unpaid miners and zero-share miners are shown explicitly. Theoretical waits average each miner's m*C divided by its absolute expected raw-token earning rate. First payout waits start at study entry or actual joining. Reimbursement includes BCH dust and fee, rather than BCH miner fee alone.

| Hash class | Miners | Paid miners | Zero shares | Intervals | Mean wait d | P95 wait d | Mean solo d | Mean m*C/rate d | First payout mean d | Token reimbursement % | Unpaid raw units | Reserved raw units |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| giant | 1 | 1 | 0 | 1438 | 0.101 | 0.092 | 0.040 | 0.000 | 0.012 | 0.027 | 3435397336 | 0 |
| medium | 2195 | 1130 | 0 | 1860 | 36.065 | 103.454 | 691.375 | 3.959 | 55.216 | 0.188 | 456141635702 | 0 |
| small | 2520 | 0 | 1018 | 0 | censored | censored | 58740.636 | 328.295 | censored | censored | 37033233735 | 0 |
| tiny | 2460 | 0 | 2406 | 0 | censored | censored | 5896173.020 | 33410.633 | censored | censored | 278306184 | 0 |
| very-tiny | 2824 | 0 | 2823 | 0 | censored | censored | 947397972.751 | 5204002.003 | censored | censored | 0 | 0 |

Fixed absolute probe: hash fraction 0.00010000, m=20, payouts 1, mean/P95 interval censored/censored days, theoretical threshold 1.142 days, solo 277.778 days, unpaid 215381271 raw units.

### 10000 miners, log-uniform, HoldBlockedRoots, N=1, share interval=10s

Run 162.169 days; history 100 wins / 496881895476 raw units; blocked coverage 943, blocked clipped minimum 0; permanently held raw units 5115249479309 (includes historical custody assumption); outstanding fundable roots 0; first split opportunity Some(351).
Integer check: won 24671363628228 = gross paid 19556114148919 + unpaid 5115249479309 + unallocated 0; reserved 0 is included in unpaid; reimbursement 4057000000 is included in gross paid. Outside claims 18013 emitted 87128209161273; ending vault 1976200427210499.

| Period | Opportunities | Won | Splits | Payouts/opportunity | Payouts/won | Eligible rank>N mean/max | Fee/split sats | Gross front/split sats | Net external/split sats |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| first 100 | 100 | 100 | 0 | 0.000 | 0.000 | 1986.58/2147 | 0.0 | 0.0 | 0.0 |
| later | 4900 | 4900 | 4057 | 0.828 | 0.828 | 3419.52/4071 | 1436.0 | 2816.0 | 2116.0 |

Waits pool completed payout intervals within class (event weighted); theory and solo columns average miners (miner weighted), so their ratio is not an unbiased speedup. Unpaid miners and zero-share miners are shown explicitly. Theoretical waits average each miner's m*C divided by its absolute expected raw-token earning rate. First payout waits start at study entry or actual joining. Reimbursement includes BCH dust and fee, rather than BCH miner fee alone.

| Hash class | Miners | Paid miners | Zero shares | Intervals | Mean wait d | P95 wait d | Mean solo d | Mean m*C/rate d | First payout mean d | Token reimbursement % | Unpaid raw units | Reserved raw units |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| high | 318 | 318 | 0 | 1791 | 20.316 | 27.898 | 20.784 | 0.119 | 18.563 | 0.021 | 735142072211 | 0 |
| medium | 2520 | 875 | 0 | 1073 | 44.066 | 75.767 | 587.498 | 3.392 | 69.398 | 0.021 | 4244774939977 | 0 |
| small | 2460 | 0 | 894 | 0 | censored | censored | 56170.578 | 321.591 | censored | censored | 133826603526 | 0 |
| tiny | 2568 | 0 | 2501 | 0 | censored | censored | 6109279.423 | 34206.717 | censored | censored | 1451238781 | 0 |
| very-tiny | 2134 | 0 | 2131 | 0 | censored | censored | 370672804.550 | 2048845.804 | censored | censored | 54624814 | 0 |

Fixed absolute probe: hash fraction 0.00010000, m=20, payouts 0, mean/P95 interval censored/censored days, theoretical threshold 1.149 days, solo 277.778 days, unpaid 3474034902 raw units.

### 10000 miners, giant70, HoldBlockedRoots, N=64, share interval=60s

Run 144.907 days; history 100 wins / 496874387642 raw units; blocked coverage 0, blocked clipped minimum 70; permanently held raw units 835850980032 (includes historical custody assumption); outstanding fundable roots 0; first split opportunity Some(1).
Integer check: won 24734831189163 = gross paid 23898980209131 + unpaid 835850980032 + unallocated 0; reserved 0 is included in unpaid; reimbursement 13932000000 is included in gross paid. Outside claims 15964 emitted 77400206520504; ending vault 1985864962290333.

| Period | Opportunities | Won | Splits | Payouts/opportunity | Payouts/won | Eligible rank>N mean/max | Fee/split sats | Gross front/split sats | Net external/split sats |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| first 100 | 100 | 100 | 99 | 0.990 | 0.990 | 704.61/866 | 1436.0 | 2816.0 | 2116.0 |
| later | 4900 | 4900 | 4831 | 2.823 | 2.823 | 2100.64/2668 | 1676.4 | 4370.1 | 3670.1 |

Waits pool completed payout intervals within class (event weighted); theory and solo columns average miners (miner weighted), so their ratio is not an unbiased speedup. Unpaid miners and zero-share miners are shown explicitly. Theoretical waits average each miner's m*C divided by its absolute expected raw-token earning rate. First payout waits start at study entry or actual joining. Reimbursement includes BCH dust and fee, rather than BCH miner fee alone.

| Hash class | Miners | Paid miners | Zero shares | Intervals | Mean wait d | P95 wait d | Mean solo d | Mean m*C/rate d | First payout mean d | Token reimbursement % | Unpaid raw units | Reserved raw units |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| giant | 1 | 1 | 0 | 4929 | 0.029 | 0.088 | 0.040 | 0.000 | 0.008 | 0.027 | 3361215564 | 0 |
| medium | 2195 | 1545 | 57 | 7454 | 15.053 | 39.907 | 691.375 | 3.973 | 39.383 | 0.152 | 717097479562 | 0 |
| small | 2520 | 3 | 1898 | 0 | censored | censored | 58740.636 | 329.517 | 115.403 | 0.159 | 114446177798 | 0 |
| tiny | 2460 | 0 | 2452 | 0 | censored | censored | 5896173.020 | 33534.927 | censored | censored | 946107108 | 0 |
| very-tiny | 2824 | 0 | 2824 | 0 | censored | censored | 947397972.751 | 5223361.786 | censored | censored | 0 | 0 |

Fixed absolute probe: hash fraction 0.00010000, m=20, payouts 3, mean/P95 interval 38.813/44.319 days, theoretical threshold 1.146 days, solo 277.778 days, unpaid 678167512 raw units.

### 10000 miners, giant70, HoldBlockedRoots, N=64, share interval=10s

Run 145.516 days; history 1 wins / 4971353820 raw units; blocked coverage 1, blocked clipped minimum 85; permanently held raw units 427055955302 (includes historical custody assumption); outstanding fundable roots 0; first split opportunity Some(2).
Integer check: won 24258825365827 = gross paid 23831769410525 + unpaid 427055955302 + unallocated 0; reserved 0 is included in unpaid; reimbursement 31166000000 is included in gross paid. Outside claims 15715 emitted 76230111069516; ending vault 1987511063564657.

| Period | Opportunities | Won | Splits | Payouts/opportunity | Payouts/won | Eligible rank>N mean/max | Fee/split sats | Gross front/split sats | Net external/split sats |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| first 100 | 100 | 100 | 82 | 6.500 | 6.500 | 625.88/1052 | 2341.5 | 8604.9 | 7904.9 |
| later | 4900 | 4900 | 4832 | 6.228 | 6.228 | 2479.28/3077 | 2128.5 | 7255.8 | 6555.8 |

Waits pool completed payout intervals within class (event weighted); theory and solo columns average miners (miner weighted), so their ratio is not an unbiased speedup. Unpaid miners and zero-share miners are shown explicitly. Theoretical waits average each miner's m*C divided by its absolute expected raw-token earning rate. First payout waits start at study entry or actual joining. Reimbursement includes BCH dust and fee, rather than BCH miner fee alone.

| Hash class | Miners | Paid miners | Zero shares | Intervals | Mean wait d | P95 wait d | Mean solo d | Mean m*C/rate d | First payout mean d | Token reimbursement % | Unpaid raw units | Reserved raw units |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| giant | 1 | 1 | 0 | 4913 | 0.030 | 0.088 | 0.040 | 0.000 | 0.066 | 0.028 | 3369477633 | 0 |
| medium | 2195 | 1989 | 0 | 24236 | 7.278 | 22.049 | 691.375 | 3.971 | 27.282 | 0.424 | 312451970906 | 0 |
| small | 2520 | 27 | 1023 | 0 | censored | censored | 58740.636 | 329.301 | 122.704 | 0.423 | 110109381696 | 0 |
| tiny | 2460 | 0 | 2405 | 0 | censored | censored | 5896173.020 | 33512.946 | censored | censored | 1109437402 | 0 |
| very-tiny | 2824 | 0 | 2823 | 0 | censored | censored | 947397972.751 | 5219938.129 | censored | censored | 15687665 | 0 |

Fixed absolute probe: hash fraction 0.00010000, m=20, payouts 9, mean/P95 interval 15.506/20.464 days, theoretical threshold 1.145 days, solo 277.778 days, unpaid 137159333 raw units.

### 10000 miners, giant70, HoldBlockedRoots, N=64, share interval=10s

Run 145.619 days; history 10 wins / 49711777224 raw units; blocked coverage 0, blocked clipped minimum 78; permanently held raw units 431601271583 (includes historical custody assumption); outstanding fundable roots 0; first split opportunity Some(1).
Integer check: won 24301412510780 = gross paid 23869811239197 + unpaid 431601271583 + unallocated 0; reserved 0 is included in unpaid; reimbursement 31616000000 is included in gross paid. Outside claims 15745 emitted 76372068914510; ending vault 1987326518574710.

| Period | Opportunities | Won | Splits | Payouts/opportunity | Payouts/won | Eligible rank>N mean/max | Fee/split sats | Gross front/split sats | Net external/split sats |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| first 100 | 100 | 100 | 88 | 4.110 | 4.110 | 756.10/1133 | 1915.3 | 5883.0 | 5183.0 |
| later | 4900 | 4900 | 4834 | 6.368 | 6.368 | 2482.68/3104 | 2146.8 | 7372.8 | 6672.8 |

Waits pool completed payout intervals within class (event weighted); theory and solo columns average miners (miner weighted), so their ratio is not an unbiased speedup. Unpaid miners and zero-share miners are shown explicitly. Theoretical waits average each miner's m*C divided by its absolute expected raw-token earning rate. First payout waits start at study entry or actual joining. Reimbursement includes BCH dust and fee, rather than BCH miner fee alone.

| Hash class | Miners | Paid miners | Zero shares | Intervals | Mean wait d | P95 wait d | Mean solo d | Mean m*C/rate d | First payout mean d | Token reimbursement % | Unpaid raw units | Reserved raw units |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| giant | 1 | 1 | 0 | 4921 | 0.030 | 0.088 | 0.040 | 0.000 | 0.056 | 0.028 | 3341773131 | 0 |
| medium | 2195 | 1990 | 0 | 24675 | 7.166 | 21.853 | 691.375 | 3.971 | 26.779 | 0.431 | 317654562605 | 0 |
| small | 2520 | 29 | 1024 | 0 | censored | censored | 58740.636 | 329.330 | 121.660 | 0.405 | 109479810780 | 0 |
| tiny | 2460 | 0 | 2405 | 0 | censored | censored | 5896173.020 | 33515.922 | censored | censored | 1109437402 | 0 |
| very-tiny | 2824 | 0 | 2823 | 0 | censored | censored | 947397972.751 | 5220401.601 | censored | censored | 15687665 | 0 |

Fixed absolute probe: hash fraction 0.00010000, m=20, payouts 9, mean/P95 interval 15.039/19.367 days, theoretical threshold 1.145 days, solo 277.778 days, unpaid 236079636 raw units.

