// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

#[cfg(test)]
mod common;
#[cfg(test)]
mod missing_input_test;
#[cfg(test)]
mod node_swarm_test;
#[cfg(test)]
mod organise_pipeline_test;
#[cfg(test)]
mod payout_tree_bchn_test;
#[cfg(test)]
mod pruned_serving_test;
// The sim build shortens the PPLNS window cap to 80 shares, which leaves the
// traded share out of these tests' windows.
#[cfg(all(test, not(feature = "sim")))]
mod share_payout_rights_test;
// The sim build shortens PAYOUT_DEPTH to 100 shares, which leaves the
// 102-share-deep coins these trades spend unspendable.
#[cfg(all(test, not(feature = "sim")))]
mod share_trading_test;
#[cfg(test)]
mod share_transaction_relay_test;
#[cfg(test)]
mod stratum_server_test;
#[cfg(test)]
mod test_api_server_health_check;
