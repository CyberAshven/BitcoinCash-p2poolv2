Candidate rules, not adopted: the queue is paid in the order miners' earned balances reached their minimum; the rest of each win is paid ahead to the miners whose shares already in the window are expected to earn the most, never beyond that expectation; a leftover too small for the next miner's minimum goes to the entry before it; no 4,096-script window stop; no launch history or custody. A list that cannot be built leaves that win unmined (strict). Seed `0x70686f746f6e6d70`; 5,000 potential pool wins per run; C=1,000,000 raw units; multiples uniform in 10/20/30/50; churn and delayed funding as in the written-rules study. Windows hold 12 hours of full-pool shares (4,320 at 10 s, 43,200 at 1 s), except the last three runs: 48 hours (17,280 at 10 s). BCH costs serialize each list through the version 2 (share-chain order) lean builder with 35-byte payees. Reproduce with `cargo photon-payout-sim --candidate-summary`; full per-period tables with `--candidate`.

| Population | Miners | N | Share s | Window h | Won/blocked (by N) | Payouts/won | Queue beyond N mean/max | Fee/payee sats | Peak advanced (rewards) | End advanced (rewards) | Departed advanced (rewards) | Beyond own bound (rewards) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| giant70 | 1000 | 64 | 10 | 12 | 5000/0 (0) | 44.338 | 0.0/1 | 158.8 | 1.805 | 1.722 | 0.000 | 0.0000 |
| log-uniform | 1000 | 64 | 10 | 12 | 5000/0 (0) | 63.941 | 30.6/57 | 150.3 | 2.311 | 2.237 | 0.000 | 1.5808 |
| heavy-tail | 1000 | 64 | 10 | 12 | 5000/0 (0) | 63.933 | 132.8/196 | 150.3 | 3.438 | 3.159 | 0.000 | 1.9561 |
| giant70 | 10000 | 64 | 10 | 12 | 4920/80 (6) | 46.930 | 0.7/35 | 157.3 | 7.440 | 7.253 | 0.010 | 8.6818 |
| log-uniform | 10000 | 64 | 10 | 12 | 73/4927 (4927) | 63.219 | 496.1/533 | 150.5 | 8.023 | 7.778 | 0.684 | 0.0673 |
| heavy-tail | 10000 | 64 | 10 | 12 | 20/4980 (4980) | 61.050 | 325.8/341 | 151.2 | 6.836 | 6.836 | 0.385 | 0.0536 |
| giant70 | 1000 | 187 | 10 | 12 | 5000/0 (0) | 44.338 | 0.0/0 | 158.8 | 1.805 | 1.722 | 0.000 | 0.0000 |
| log-uniform | 1000 | 187 | 10 | 12 | 5000/0 (0) | 83.567 | 0.0/0 | 145.8 | 2.126 | 2.031 | 0.000 | 2.7557 |
| heavy-tail | 1000 | 187 | 10 | 12 | 5000/0 (0) | 109.883 | 0.0/0 | 142.2 | 2.905 | 2.653 | 0.000 | 2.6551 |
| giant70 | 10000 | 187 | 10 | 12 | 4943/57 (0) | 47.114 | 0.0/2 | 157.2 | 7.422 | 7.242 | 0.005 | 8.7902 |
| log-uniform | 10000 | 187 | 10 | 12 | 4620/380 (50) | 121.722 | 0.1/13 | 141.1 | 7.033 | 6.801 | 0.022 | 6.3239 |
| heavy-tail | 10000 | 187 | 10 | 12 | 3320/1680 (140) | 120.727 | 0.5/14 | 141.2 | 10.653 | 10.602 | 0.719 | 3.9139 |
| giant70 | 10000 | 187 | 1 | 12 | 1720/3280 (0) | 58.410 | 0.0/2 | 152.1 | 6.897 | 6.891 | 0.039 | 2.1938 |
| log-uniform | 10000 | 187 | 1 | 12 | 906/4094 (1092) | 168.599 | 0.9/19 | 138.3 | 6.332 | 6.329 | 0.388 | 0.8205 |
| heavy-tail | 10000 | 187 | 1 | 12 | 16/4984 (86) | 128.125 | 7.0/9 | 140.6 | 5.438 | 5.438 | 0.334 | 0.0189 |
| giant70 | 10000 | 187 | 10 | 48 | 5000/0 (0) | 63.635 | 0.0/0 | 150.4 | 11.098 | 10.852 | 0.000 | 0.0000 |
| log-uniform | 10000 | 187 | 10 | 48 | 5000/0 (0) | 129.071 | 0.0/0 | 140.6 | 12.086 | 11.933 | 0.000 | 2.4214 |
| heavy-tail | 10000 | 187 | 10 | 48 | 5000/0 (0) | 119.600 | 0.0/0 | 141.3 | 19.887 | 19.568 | 0.039 | 7.8928 |

Blocked wins had no valid list, so the pool could not mine them; "by N" counts those that all offers together could have covered, so only the payee cap blocked them. The others lacked enough earned balance plus bounded advance in total. Payouts/won and fee/payee cover the whole run, including the first 100 wins. Queue beyond N counts queued miners not listed in a win, after the first 100. Advanced balances are tokens paid ahead and not yet recovered from later credit, in mean won rewards; departed advances are recovered only if those miners return. Beyond own bound is paid above a miner's earned balance plus expected credit, only by absorbing a leftover smaller than the next minimum.

| Population | Miners | N | Share s | Window h | Class | Paid/total | Zero shares | Mean/P95 interval days | Mean m*C/rate days | First payout mean days | Unpaid raw tokens |
|---|---:|---:|---:|---:|---|---:|---:|---:|---:|---:|---:|
| giant70 | 1000 | 64 | 10 | 12 | giant | 1/1 | 0 | 0.029/0.087 | 0.000 | 0.013 | 0 |
| giant70 | 1000 | 64 | 10 | 12 | high | 94/94 | 0 | 0.070/0.197 | 0.077 | 0.147 | 2250561447 |
| giant70 | 1000 | 64 | 10 | 12 | medium | 257/257 | 0 | 0.599/2.210 | 3.780 | 5.381 | 3780160352 |
| giant70 | 1000 | 64 | 10 | 12 | small | 116/246 | 91 | 13.166/46.152 | 335.775 | 47.203 | 2220179447 |
| giant70 | 1000 | 64 | 10 | 12 | tiny | 1/242 | 236 | censored/censored | 33318.446 | 47.183 | 103544472 |
| giant70 | 1000 | 64 | 10 | 12 | very-tiny | 0/160 | 160 | censored/censored | 898741.641 | censored | 0 |
| log-uniform | 1000 | 64 | 10 | 12 | high | 150/150 | 0 | 0.080/0.242 | 0.050 | 0.122 | 4450175158 |
| log-uniform | 1000 | 64 | 10 | 12 | medium | 273/274 | 0 | 0.635/2.338 | 3.414 | 4.749 | 4313084393 |
| log-uniform | 1000 | 64 | 10 | 12 | small | 106/233 | 91 | 17.954/63.197 | 336.595 | 48.398 | 1975272983 |
| log-uniform | 1000 | 64 | 10 | 12 | tiny | 1/256 | 251 | censored/censored | 37236.560 | 132.209 | 90270198 |
| log-uniform | 1000 | 64 | 10 | 12 | very-tiny | 0/87 | 87 | censored/censored | 414549.796 | censored | 0 |
| heavy-tail | 1000 | 64 | 10 | 12 | giant | 1/1 | 0 | 0.033/0.098 | 0.001 | 0.014 | 0 |
| heavy-tail | 1000 | 64 | 10 | 12 | high | 143/143 | 0 | 0.165/0.399 | 0.081 | 0.195 | 4022618058 |
| heavy-tail | 1000 | 64 | 10 | 12 | medium | 656/656 | 0 | 0.448/1.287 | 0.462 | 0.820 | 11263399046 |
| heavy-tail | 1000 | 64 | 10 | 12 | tiny | 0/70 | 70 | censored/censored | 63173.619 | censored | 0 |
| heavy-tail | 1000 | 64 | 10 | 12 | very-tiny | 0/130 | 130 | censored/censored | 12971277.301 | censored | 0 |
| giant70 | 10000 | 64 | 10 | 12 | giant | 1/1 | 0 | 0.030/0.088 | 0.000 | 0.056 | 0 |
| giant70 | 10000 | 64 | 10 | 12 | medium | 2190/2195 | 0 | 1.161/4.244 | 3.971 | 5.650 | 17236289842 |
| giant70 | 10000 | 64 | 10 | 12 | small | 1119/2520 | 1022 | 15.559/60.731 | 329.292 | 49.073 | 17187333201 |
| giant70 | 10000 | 64 | 10 | 12 | tiny | 21/2460 | 2405 | 0.207/0.286 | 33512.020 | 83.254 | 754961751 |
| giant70 | 10000 | 64 | 10 | 12 | very-tiny | 0/2824 | 2823 | censored/censored | 5219793.796 | censored | 15687665 |
| log-uniform | 10000 | 64 | 10 | 12 | high | 316/318 | 0 | 23.061/78.206 | 0.118 | 2.534 | 3507751465 |
| log-uniform | 10000 | 64 | 10 | 12 | medium | 1254/2520 | 0 | 48.945/135.698 | 3.368 | 42.774 | 32717072334 |
| log-uniform | 10000 | 64 | 10 | 12 | small | 4/2460 | 892 | 2.921/3.334 | 319.381 | 14.816 | 1803174639 |
| log-uniform | 10000 | 64 | 10 | 12 | tiny | 0/2568 | 2503 | censored/censored | 33971.620 | censored | 16884369 |
| log-uniform | 10000 | 64 | 10 | 12 | very-tiny | 0/2134 | 2131 | censored/censored | 2034764.422 | censored | 0 |
| heavy-tail | 10000 | 64 | 10 | 12 | giant | 1/1 | 0 | 0.336/1.057 | 0.001 | 0.123 | 0 |
| heavy-tail | 10000 | 64 | 10 | 12 | high | 109/109 | 0 | 24.673/124.262 | 0.085 | 1.962 | 853283648 |
| heavy-tail | 10000 | 64 | 10 | 12 | medium | 677/7890 | 0 | 53.810/128.554 | 4.101 | 54.333 | 32701667931 |
| heavy-tail | 10000 | 64 | 10 | 12 | tiny | 0/153 | 152 | censored/censored | 109942.068 | censored | 0 |
| heavy-tail | 10000 | 64 | 10 | 12 | very-tiny | 0/1847 | 1846 | censored/censored | 93244060.308 | censored | 0 |
| giant70 | 1000 | 187 | 10 | 12 | giant | 1/1 | 0 | 0.029/0.087 | 0.000 | 0.013 | 0 |
| giant70 | 1000 | 187 | 10 | 12 | high | 94/94 | 0 | 0.070/0.197 | 0.077 | 0.147 | 2250561447 |
| giant70 | 1000 | 187 | 10 | 12 | medium | 257/257 | 0 | 0.599/2.210 | 3.780 | 5.381 | 3780160352 |
| giant70 | 1000 | 187 | 10 | 12 | small | 116/246 | 91 | 13.166/46.152 | 335.775 | 47.203 | 2220179447 |
| giant70 | 1000 | 187 | 10 | 12 | tiny | 1/242 | 236 | censored/censored | 33318.446 | 47.183 | 103544472 |
| giant70 | 1000 | 187 | 10 | 12 | very-tiny | 0/160 | 160 | censored/censored | 898741.641 | censored | 0 |
| log-uniform | 1000 | 187 | 10 | 12 | high | 150/150 | 0 | 0.060/0.202 | 0.050 | 0.115 | 3771772594 |
| log-uniform | 1000 | 187 | 10 | 12 | medium | 273/274 | 0 | 0.563/2.093 | 3.414 | 4.724 | 3917279151 |
| log-uniform | 1000 | 187 | 10 | 12 | small | 106/233 | 91 | 16.810/60.926 | 336.595 | 48.371 | 2048179819 |
| log-uniform | 1000 | 187 | 10 | 12 | tiny | 1/256 | 251 | censored/censored | 37236.560 | 132.183 | 91371674 |
| log-uniform | 1000 | 187 | 10 | 12 | very-tiny | 0/87 | 87 | censored/censored | 414549.796 | censored | 0 |
| heavy-tail | 1000 | 187 | 10 | 12 | giant | 1/1 | 0 | 0.033/0.098 | 0.001 | 0.014 | 0 |
| heavy-tail | 1000 | 187 | 10 | 12 | high | 143/143 | 0 | 0.073/0.225 | 0.081 | 0.170 | 2900902453 |
| heavy-tail | 1000 | 187 | 10 | 12 | medium | 656/656 | 0 | 0.314/1.045 | 0.462 | 0.759 | 9939503823 |
| heavy-tail | 1000 | 187 | 10 | 12 | tiny | 0/70 | 70 | censored/censored | 63173.619 | censored | 0 |
| heavy-tail | 1000 | 187 | 10 | 12 | very-tiny | 0/130 | 130 | censored/censored | 12971277.301 | censored | 0 |
| giant70 | 10000 | 187 | 10 | 12 | giant | 1/1 | 0 | 0.029/0.087 | 0.000 | 0.056 | 0 |
| giant70 | 10000 | 187 | 10 | 12 | medium | 2190/2195 | 0 | 1.152/4.223 | 3.971 | 5.648 | 17217839675 |
| giant70 | 10000 | 187 | 10 | 12 | small | 1123/2520 | 1022 | 15.397/61.168 | 329.293 | 49.251 | 17139647260 |
| giant70 | 10000 | 187 | 10 | 12 | tiny | 21/2460 | 2405 | 0.200/0.286 | 33512.167 | 83.289 | 764866231 |
| giant70 | 10000 | 187 | 10 | 12 | very-tiny | 0/2824 | 2823 | censored/censored | 5219816.758 | censored | 15687665 |
| log-uniform | 10000 | 187 | 10 | 12 | high | 318/318 | 0 | 0.207/0.542 | 0.118 | 0.199 | 316703269 |
| log-uniform | 10000 | 187 | 10 | 12 | medium | 2519/2520 | 0 | 0.976/3.463 | 3.387 | 4.705 | 16853631501 |
| log-uniform | 10000 | 187 | 10 | 12 | small | 1211/2460 | 892 | 21.464/70.802 | 321.177 | 50.927 | 15053892071 |
| log-uniform | 10000 | 187 | 10 | 12 | tiny | 31/2568 | 2503 | 0.185/0.306 | 34162.666 | 78.903 | 705639825 |
| log-uniform | 10000 | 187 | 10 | 12 | very-tiny | 1/2134 | 2131 | 0.133/0.133 | 2046207.342 | 93.917 | 34546032 |
| heavy-tail | 10000 | 187 | 10 | 12 | giant | 1/1 | 0 | 0.050/0.118 | 0.001 | 0.029 | 0 |
| heavy-tail | 10000 | 187 | 10 | 12 | high | 109/109 | 0 | 0.170/0.514 | 0.085 | 0.170 | 59833561 |
| heavy-tail | 10000 | 187 | 10 | 12 | medium | 7888/7890 | 0 | 3.716/14.026 | 4.117 | 6.074 | 51216879894 |
| heavy-tail | 10000 | 187 | 10 | 12 | tiny | 0/153 | 152 | censored/censored | 110380.801 | censored | 11107309 |
| heavy-tail | 10000 | 187 | 10 | 12 | very-tiny | 0/1847 | 1846 | censored/censored | 93616158.385 | censored | 12501175 |
| giant70 | 10000 | 187 | 1 | 12 | giant | 1/1 | 0 | 0.069/0.188 | 0.000 | 0.062 | 0 |
| giant70 | 10000 | 187 | 1 | 12 | medium | 2048/2195 | 0 | 1.785/6.614 | 3.961 | 4.141 | 16639736702 |
| giant70 | 10000 | 187 | 1 | 12 | small | 649/2520 | 148 | 22.209/55.093 | 328.456 | 31.549 | 16831620612 |
| giant70 | 10000 | 187 | 1 | 12 | tiny | 0/2460 | 2019 | censored/censored | 33426.999 | censored | 412262982 |
| giant70 | 10000 | 187 | 1 | 12 | very-tiny | 0/2824 | 2815 | censored/censored | 5206551.065 | censored | 5232252 |
| log-uniform | 10000 | 187 | 1 | 12 | high | 318/318 | 0 | 0.593/1.416 | 0.118 | 0.198 | 29878720 |
| log-uniform | 10000 | 187 | 1 | 12 | medium | 2357/2520 | 0 | 2.409/9.171 | 3.372 | 5.591 | 16723801395 |
| log-uniform | 10000 | 187 | 1 | 12 | small | 409/2460 | 108 | 44.087/81.373 | 319.759 | 49.502 | 14104377757 |
| log-uniform | 10000 | 187 | 1 | 12 | tiny | 0/2568 | 2062 | censored/censored | 34011.849 | censored | 241610480 |
| log-uniform | 10000 | 187 | 1 | 12 | very-tiny | 0/2134 | 2123 | censored/censored | 2037174.030 | censored | 3309646 |
| heavy-tail | 10000 | 187 | 1 | 12 | giant | 1/1 | 0 | 0.266/0.266 | 0.001 | 0.115 | 0 |
| heavy-tail | 10000 | 187 | 1 | 12 | high | 108/109 | 0 | 11.047/59.724 | 0.084 | 0.161 | 114156823 |
| heavy-tail | 10000 | 187 | 1 | 12 | medium | 954/7890 | 0 | 34.782/127.314 | 4.101 | 22.103 | 26730477318 |
| heavy-tail | 10000 | 187 | 1 | 12 | tiny | 0/153 | 148 | censored/censored | 109927.756 | censored | 0 |
| heavy-tail | 10000 | 187 | 1 | 12 | very-tiny | 0/1847 | 1841 | censored/censored | 93231921.833 | censored | 113308 |
| giant70 | 10000 | 187 | 10 | 48 | giant | 1/1 | 0 | 0.029/0.087 | 0.000 | 0.056 | 0 |
| giant70 | 10000 | 187 | 10 | 48 | medium | 2189/2195 | 0 | 0.837/2.832 | 3.971 | 5.961 | 31096272001 |
| giant70 | 10000 | 187 | 10 | 48 | small | 1118/2520 | 1022 | 14.166/58.359 | 329.298 | 49.309 | 20789059556 |
| giant70 | 10000 | 187 | 10 | 48 | tiny | 22/2460 | 2405 | 0.932/1.181 | 33512.616 | 86.052 | 742632846 |
| giant70 | 10000 | 187 | 10 | 48 | very-tiny | 0/2824 | 2823 | censored/censored | 5219886.651 | censored | 14843903 |
| log-uniform | 10000 | 187 | 10 | 48 | high | 318/318 | 0 | 0.484/1.452 | 0.118 | 0.272 | 639881228 |
| log-uniform | 10000 | 187 | 10 | 48 | medium | 2520/2520 | 0 | 0.609/1.878 | 3.388 | 5.077 | 34287898507 |
| log-uniform | 10000 | 187 | 10 | 48 | small | 1218/2460 | 892 | 15.526/58.895 | 321.237 | 52.637 | 21999014436 |
| log-uniform | 10000 | 187 | 10 | 48 | tiny | 33/2568 | 2503 | 1.085/1.374 | 34169.111 | 78.269 | 798718108 |
| log-uniform | 10000 | 187 | 10 | 48 | very-tiny | 1/2134 | 2131 | 1.247/1.247 | 2046593.388 | 94.613 | 34258655 |
| heavy-tail | 10000 | 187 | 10 | 48 | giant | 1/1 | 0 | 0.031/0.094 | 0.001 | 0.010 | 0 |
| heavy-tail | 10000 | 187 | 10 | 48 | high | 109/109 | 0 | 0.176/0.592 | 0.085 | 0.201 | 227712418 |
| heavy-tail | 10000 | 187 | 10 | 48 | medium | 7889/7890 | 0 | 2.232/8.480 | 4.125 | 6.428 | 94179740663 |
| heavy-tail | 10000 | 187 | 10 | 48 | tiny | 0/153 | 152 | censored/censored | 110588.531 | censored | 17169097 |
| heavy-tail | 10000 | 187 | 10 | 48 | very-tiny | 0/1847 | 1846 | censored/censored | 93792338.098 | censored | 20147841 |

Intervals are completed payout-to-payout waits, event weighted; m*C/rate is the threshold wait averaged over miners. `censored` means no completed interval within about 139 days, not a measured infinite wait. Zero-share miners found no share during the run.

| Population | Miners | N | Share s | Window h | Probe payouts | Mean/P95 interval days | m*C/earning-rate days | Unpaid raw tokens |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| giant70 | 1000 | 64 | 10 | 12 | 124 | 1.162/3.492 | 1.145 | 4381848 |
| log-uniform | 1000 | 64 | 10 | 12 | 164 | 0.987/3.026 | 1.148 | 16353911 |
| heavy-tail | 1000 | 64 | 10 | 12 | 145 | 1.123/3.492 | 1.148 | 0 |
| giant70 | 10000 | 64 | 10 | 12 | 92 | 1.538/4.043 | 1.145 | 18490232 |
| log-uniform | 10000 | 64 | 10 | 12 | 1 | censored/censored | 1.141 | 7754209 |
| heavy-tail | 10000 | 64 | 10 | 12 | 0 | censored/censored | 1.144 | 5719776 |
| giant70 | 1000 | 187 | 10 | 12 | 124 | 1.162/3.492 | 1.145 | 4381848 |
| log-uniform | 1000 | 187 | 10 | 12 | 164 | 0.987/3.026 | 1.148 | 16353911 |
| heavy-tail | 1000 | 187 | 10 | 12 | 164 | 0.992/2.974 | 1.148 | 11991118 |
| giant70 | 10000 | 187 | 10 | 12 | 92 | 1.538/3.920 | 1.145 | 19584979 |
| log-uniform | 10000 | 187 | 10 | 12 | 109 | 1.477/3.713 | 1.148 | 12894054 |
| heavy-tail | 10000 | 187 | 10 | 12 | 88 | 2.047/4.745 | 1.149 | 0 |
| giant70 | 10000 | 187 | 1 | 12 | 41 | 2.400/5.778 | 1.142 | 5147849 |
| log-uniform | 10000 | 187 | 1 | 12 | 24 | 4.651/8.732 | 1.143 | 4100442 |
| heavy-tail | 10000 | 187 | 1 | 12 | 1 | censored/censored | 1.144 | 0 |
| giant70 | 10000 | 187 | 10 | 48 | 117 | 1.216/3.040 | 1.145 | 10683075 |
| log-uniform | 10000 | 187 | 10 | 48 | 163 | 0.984/2.180 | 1.148 | 17176836 |
| heavy-tail | 10000 | 187 | 10 | 48 | 144 | 1.241/3.054 | 1.151 | 18458510 |

The probe keeps 0.0001 of the original full pool's hash and m=20 in every run, so its threshold wait is the same everywhere; a population-independent rule gives it the same observed wait at 1,000 and 10,000 miners.
