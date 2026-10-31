The following seeded economic diagnostics use 100 historical won rewards held under an unspecified bootstrap custody rule. They are not an approved bootstrap or a deployable protocol. Main runs cover 5,000 further potential wins. Permanently blocked R lists are never repaired. Full inputs, exact integer checks, censoring, and BCH serialization methods are in [simulation-results.md](simulation-results.md). Reproduce these compact tables with `cargo photon-payout-sim --summary`.

Token C=1,000,000 raw units; multipliers 10/20/30/50; N=64; full pool share/win means 10/2,400 seconds; outside claims mean 800 seconds. The six distributions include 1,000/10,000 miners, one 70-percent giant, eight-decade log-uniform hash rates, and a Pareto/microscopic mix. Churn retains old shares and debts. All parameters are scenario assumptions.

| Population | Miners | Class | Paid/total | Mean/P95 interval days | Mean solo days | Token reimbursement % | Unpaid raw tokens |
|---|---:|---|---:|---:|---:|---:|---:|
| giant70 | 1000 | giant | 1/1 | 0.039/0.116 | 0.040 | 0.021 | 6437967634 |
| giant70 | 1000 | high | 94/94 | 9.191/21.431 | 13.363 | 0.021 | 230895713031 |
| giant70 | 1000 | medium | 79/257 | 29.891/67.868 | 602.303 | 0.024 | 405130751410 |
| giant70 | 1000 | small | 0/246 | censored/censored | 57803.696 | censored | 13183696928 |
| giant70 | 1000 | tiny | 0/242 | censored/censored | 6265193.915 | censored | 126164359 |
| giant70 | 1000 | very-tiny | 0/160 | censored/censored | 142787836.410 | censored | 0 |
| log-uniform | 1000 | high | 150/150 | 2.272/6.710 | 8.575 | 0.039 | 209594443391 |
| log-uniform | 1000 | medium | 135/274 | 21.447/47.171 | 587.205 | 0.046 | 290218239029 |
| log-uniform | 1000 | small | 0/233 | censored/censored | 55337.249 | censored | 11841165226 |
| log-uniform | 1000 | tiny | 0/256 | censored/censored | 6026253.083 | censored | 102393696 |
| log-uniform | 1000 | very-tiny | 0/87 | censored/censored | 66155785.076 | censored | 0 |
| heavy-tail | 1000 | giant | 1/1 | 0.068/0.161 | 0.172 | 0.053 | 831816527 |
| heavy-tail | 1000 | high | 143/143 | 2.012/6.858 | 14.896 | 0.069 | 123687610673 |
| heavy-tail | 1000 | medium | 656/656 | 16.086/36.005 | 77.410 | 0.081 | 556185910424 |
| heavy-tail | 1000 | tiny | 0/70 | censored/censored | 9146399.273 | censored | 0 |
| heavy-tail | 1000 | very-tiny | 0/130 | censored/censored | 2232137981.665 | censored | 0 |
| giant70 | 10000 | giant | 1/1 | 0.029/0.087 | 0.040 | 0.028 | 3406393320 |
| giant70 | 10000 | medium | 1737/2195 | 11.408/30.331 | 691.375 | 0.220 | 532481931389 |
| giant70 | 10000 | small | 0/2520 | censored/censored | 58740.636 | censored | 118690593356 |
| giant70 | 10000 | tiny | 0/2460 | censored/censored | 5896173.020 | censored | 1093562644 |
| giant70 | 10000 | very-tiny | 0/2824 | censored/censored | 947397972.751 | censored | 15687665 |
| log-uniform | 10000 | high | 318/318 | 2.572/3.877 | 20.784 | 0.163 | 93425761511 |
| log-uniform | 10000 | medium | 1994/2520 | 9.671/28.634 | 587.498 | 0.166 | 765150345026 |
| log-uniform | 10000 | small | 0/2460 | censored/censored | 56170.578 | censored | 133826603526 |
| log-uniform | 10000 | tiny | 0/2568 | censored/censored | 6109279.423 | censored | 1451238781 |
| log-uniform | 10000 | very-tiny | 0/2134 | censored/censored | 370672804.550 | censored | 54624814 |
| heavy-tail | 10000 | giant | 1/1 | 0.031/0.095 | 0.241 | 0.166 | 562848606 |
| heavy-tail | 10000 | high | 109/109 | 0.746/2.010 | 15.628 | 0.264 | 22174604102 |
| heavy-tail | 10000 | medium | 6998/7890 | 18.954/68.729 | 730.158 | 0.299 | 1620969521514 |
| heavy-tail | 10000 | tiny | 0/153 | censored/censored | 19962327.532 | censored | 12190925 |
| heavy-tail | 10000 | very-tiny | 0/1847 | censored/censored | 16415371799.757 | censored | 20438351 |

Completed intervals are event weighted; solo is miner weighted. Broad-class ratios are not unbiased speedups. `censored` means no completed interval, not an infinite measured wait. Paid/total counts include miners with just one payout; most microscopic miners found no shares at all. Token reimbursement C/gross includes output dust and fee, not only BCH miner fee.

Bootstrap is not solved: a strict zero-history probe rejects all 100 potential pool wins and credits no debt. For conditional runs, the first 100 synthetic historical wins supply about 4.97e11 raw units; their assumed custody is unresolved. The first 100 subsequent opportunities are separated from later operation below.

| Population | Miners | Period | Won/splits | Payouts/won | Eligible rank>N mean/max | Fee/split sats | Gross/net fronting sats |
|---|---:|---|---:|---:|---:|---:|---:|
| giant70 | 1000 | first 100 | 100/99 | 0.990 | 184.85/204 | 1436.0 | 2816.0/2116.0 |
| giant70 | 1000 | later 4,900 | 4900/4868 | 1.008 | 334.42/405 | 1437.9 | 2828.2/2128.2 |
| log-uniform | 1000 | first 100 | 100/99 | 0.990 | 246.47/267 | 1436.0 | 2816.0/2116.0 |
| log-uniform | 1000 | later 4,900 | 4900/4898 | 1.924 | 397.56/465 | 1553.5 | 3585.9/2885.9 |
| heavy-tail | 1000 | first 100 | 100/97 | 1.100 | 634.69/636 | 1453.4 | 2927.9/2227.9 |
| heavy-tail | 1000 | later 4,900 | 4900/4865 | 3.359 | 662.23/735 | 1744.2 | 4804.5/4104.5 |
| giant70 | 10000 | first 100 | 100/99 | 0.990 | 1335.17/1478 | 1436.0 | 2816.0/2116.0 |
| giant70 | 10000 | later 4,900 | 4900/4868 | 3.732 | 2615.27/3194 | 1793.3 | 5116.5/4416.5 |
| log-uniform | 10000 | first 100 | 100/98 | 6.070 | 1903.71/2051 | 2112.4 | 7154.1/6454.1 |
| log-uniform | 10000 | later 4,900 | 4900/4800 | 7.831 | 3280.13/3902 | 2348.3 | 8659.3/7959.3 |
| heavy-tail | 10000 | first 100 | 100/95 | 5.830 | 4266.46/4922 | 2105.4 | 7106.9/6406.9 |
| heavy-tail | 10000 | later 4,900 | 4900/4670 | 13.010 | 6718.88/7494 | 3089.2 | 13387.8/12687.8 |

Backlog counts eligible miners whose rank exceeds N. It does not establish that N caused delay: a reward can run out before N. Gross fronting is fee plus every token-output BCH minimum; net fronting credits the root's inherited 700 sats. Costs serialize each actual list/amount through the lean builder with 35-byte beneficiary scripts and one fixed-length funding unlock. A delayed split can settle in a later period than its win.

The population-independence probe keeps the same absolute hash rate (0.0001 of the original full pool) and m=20, so its solo expected wait is 277.778 days at both population sizes.

| Population | Miners | Probe payouts | Mean/P95 interval days | m*C/earning-rate days | Unpaid raw tokens |
|---|---:|---:|---:|---:|---:|
| giant70 | 1000 | 0 | censored/censored | 1.146 | 2616951161 |
| log-uniform | 1000 | 1 | censored/censored | 1.149 | 1378070570 |
| heavy-tail | 1000 | 2 | 76.940/76.940 | 1.149 | 542243409 |
| giant70 | 10000 | 5 | 25.265/27.892 | 1.146 | 224818407 |
| log-uniform | 10000 | 5 | 29.919/32.954 | 1.149 | 497551214 |
| heavy-tail | 10000 | 11 | 15.272/21.830 | 1.152 | 368679123 |

| Sensitivity | Won | Funded splits | Coverage/remainder failures | Probe payouts | Probe mean/P95 days | Permanently held raw tokens |
|---|---:|---:|---:|---:|---:|---:|
| strict genesis | 0 | 0 | 100/0 | 0 | censored/censored | 0 |
| strict conditional 10k giant | 1439 | 1439 | 0/3561 | 1 | censored/censored | 496888572957 |
| N=1 conditional 10k log | 5000 | 4057 | 943/0 | 0 | censored/censored | 5115249479309 |
| 60s shares conditional 10k giant | 5000 | 4930 | 0/70 | 3 | 38.813/44.319 | 835850980032 |
| history=1 conditional 10k giant | 5000 | 4914 | 1/85 | 9 | 15.506/20.464 | 427055955302 |
| history=10 conditional 10k giant | 5000 | 4922 | 0/78 | 9 | 15.039/19.367 | 431601271583 |

The threshold-wait formula assumes service near the threshold. It is not a general result for this scheduler: most-owed priority pays whole old debts, a reward may be exhausted with cap slack, and inaccessible old roots leave a persistent debt queue. The fixed probe's observed waits differ sharply from its roughly 1.15-day threshold calculation even at N=64. A cap of one, sparse share finding, a large miner's old debt, delayed funding, bootstrap, and minimum-invalid final clips add further delay. This study finds no population-independent payout-wait guarantee. The strict model rejects invalid candidates instead of inventing extra owed credit; the held-root diagnostics expose the resulting inaccessible-token and queue consequences, and must not be presented as valid pool operation.
