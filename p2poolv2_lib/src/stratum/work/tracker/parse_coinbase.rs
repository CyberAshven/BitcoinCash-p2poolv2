// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::address_display::payout_address_string;
use crate::stratum::work::coinbase::extract_outputs_from_coinbase2;
use crate::stratum::work::tracker::JobTracker;
use bitcoin::Amount;
use p2poolv2_wallet::bch_share::BchShareNetwork;
use p2poolv2_wallet::payout_address::PayoutAddress;
use std::sync::Arc;

/// Parse the coinbase in the latest job and return its paying outputs and
/// total as metric lines.
///
/// `cash_network` is set on a Bitcoin Cash node: its payouts are labelled in
/// CashAddr under that network's prefix, as operators know them. A Bitcoin
/// node passes `None` and keeps the address's own form.
pub fn get_distribution(
    tracker: &Arc<JobTracker>,
    pool_signature_length: usize,
    network: bitcoin::network::Network,
    cash_network: Option<BchShareNetwork>,
) -> Option<String> {
    let job_id = tracker.get_latest_job_id();
    let job_details = tracker.get_job(job_id)?;

    match extract_outputs_from_coinbase2(&job_details.coinbase2, pool_signature_length) {
        Ok(outputs) => {
            let total_value = job_details.blocktemplate.coinbasevalue;
            let mut exposition = String::new();

            // Use index in case the fees and donation addresses are
            // the same. This way we can leave on grafana how to show
            // them
            for (index, tx_out) in outputs.iter().enumerate() {
                if tx_out.value != Amount::ZERO {
                    match PayoutAddress::from_script(&tx_out.script_pubkey, network) {
                        Ok(address) => {
                            let address = payout_address_string(&address, cash_network);
                            exposition.push_str(&format!(
                                "coinbase_output{{index=\"{index}\",address=\"{address}\"}} {}\n",
                                tx_out.value.to_sat()
                            ));
                        }
                        Err(_) => tracing::error!("Error parsing address from coinbase"),
                    }
                }
            }

            exposition.push_str(&format!("coinbase_total {total_value}\n"));
            Some(exposition)
        }
        Err(e) => {
            tracing::error!("Failed to parse coinbase for metrics: {}", e);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stratum::work::block_template::BlockTemplate;
    use crate::stratum::work::coinbase::parse_address;
    use crate::stratum::work::tracker::start_tracker_actor;
    use crate::test_utils::TEST_COINBASE_NSECS;
    use bitcoin::hashes::Hash;
    use bitcoin::{Amount, Network, TxOut};
    use p2poolv2_wallet::bch_share::encode_payout_address;
    use std::collections::HashMap;
    use std::str::FromStr;
    use std::sync::Arc;

    fn create_test_template() -> BlockTemplate {
        BlockTemplate {
            default_witness_commitment: None,
            height: 100,
            version: 0x20000000,
            previousblockhash: "0".repeat(64),
            bits: "1d00ffff".to_string(),
            curtime: 1234567890,
            transactions: vec![],
            coinbasevalue: 5_000_000_000,
            coinbaseaux: HashMap::new(),
            rules: vec![],
            vbavailable: HashMap::new(),
            vbrequired: 0,
            longpollid: "".to_string(),
            target: "".to_string(),
            mintime: 0,
            mutable: vec![],
            noncerange: "".to_string(),
            sigoplimit: 0,
            sizelimit: 0,
            weightlimit: 0,
            provided: None,
        }
    }

    /// Build a test coinbase2 hex string matching the new scriptSig layout.
    ///
    /// coinbase2: [nsecs_push][pool_sig_push][sequence][outputs][locktime]
    /// No commitment hash push since test jobs use share_commitment = None.
    fn create_valid_coinbase2(pool_sig: &[u8], outputs: &[TxOut]) -> String {
        let mut coinbase2_bytes = Vec::new();
        // nsecs push: 0x08 opcode + 8 zero bytes
        coinbase2_bytes.push(0x08);
        coinbase2_bytes.extend_from_slice(&[0u8; 8]);
        // pool_sig push
        coinbase2_bytes.push(pool_sig.len() as u8);
        coinbase2_bytes.extend_from_slice(pool_sig);
        coinbase2_bytes.extend_from_slice(&[0xff, 0xff, 0xff, 0xff]); // Sequence
        coinbase2_bytes.extend_from_slice(&bitcoin::consensus::serialize(&outputs.to_vec()));
        coinbase2_bytes.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // LockTime
        hex::encode(coinbase2_bytes)
    }

    #[tokio::test]
    async fn test_get_distribution_success() {
        let tracker = start_tracker_actor();

        let address = parse_address(
            "tb1q3udk7r26qs32ltf9nmqrjaaa7tr55qmkk30q5d",
            Network::Signet,
        )
        .unwrap();

        let outputs = vec![TxOut {
            value: Amount::from_str("49 BTC").unwrap(),
            script_pubkey: address.script_pubkey(),
        }];

        let coinbase2 = create_valid_coinbase2(b"P2Poolv2", &outputs);

        let job_id = tracker.get_next_job_id();
        tracker.insert_job(
            Arc::new(create_test_template()),
            "".to_string(),
            coinbase2,
            None,
            TEST_COINBASE_NSECS,
            vec![],
            job_id,
        );

        let result = get_distribution(&tracker, 8, Network::Signet, None);

        assert!(result.is_some());
        let exposition = result.unwrap();

        // only one ouput in exposition - skipping the witness commitment
        assert_eq!(exposition.matches("coinbase_output").count(), 1);

        assert!(exposition.contains(
            "coinbase_output{index=\"0\",address=\"tb1q3udk7r26qs32ltf9nmqrjaaa7tr55qmkk30q5d\"} 4900000000"
        ));
        assert!(exposition.contains("coinbase_total 5000000000"));
    }

    /// A Bitcoin Cash node holds a regtest payout under the legacy regtest
    /// network, which prints as Base58. The metric must label it with the
    /// CashAddr the operator configured.
    #[tokio::test]
    async fn test_get_distribution_labels_cash_payouts_as_cashaddr() {
        let tracker = start_tracker_actor();
        let payout = bitcoin::Address::p2pkh(
            bitcoin::PubkeyHash::from_byte_array([0x22; 20]),
            Network::Regtest,
        );
        let outputs = vec![TxOut {
            value: Amount::from_sat(312_500_000),
            script_pubkey: payout.script_pubkey(),
        }];
        let job_id = tracker.get_next_job_id();
        tracker.insert_job(
            Arc::new(create_test_template()),
            "".to_string(),
            create_valid_coinbase2(b"P2Poolv2", &outputs),
            None,
            TEST_COINBASE_NSECS,
            vec![],
            job_id,
        );

        let exposition = get_distribution(
            &tracker,
            8,
            Network::Regtest,
            Some(BchShareNetwork::Regtest),
        )
        .unwrap();

        let cashaddr = encode_payout_address(&payout, BchShareNetwork::Regtest).unwrap();
        assert!(cashaddr.starts_with("bchreg:"));
        assert!(exposition.contains(&format!(
            "coinbase_output{{index=\"0\",address=\"{cashaddr}\"}} 312500000"
        )));
        assert!(!exposition.contains(&payout.to_string()));
    }

    #[tokio::test]
    async fn test_get_distribution_no_jobs_returns_none() {
        let tracker = start_tracker_actor();

        // Don't insert any jobs
        let result = get_distribution(&tracker, 8, Network::Signet, None);

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_get_distribution_invalid_coinbase2_returns_none() {
        let tracker = start_tracker_actor();

        // Insert job with invalid coinbase2 (too short)
        let job_id = tracker.get_next_job_id();
        tracker.insert_job(
            Arc::new(create_test_template()),
            "".to_string(),
            "deadbeef".to_string(), // Invalid coinbase2
            None,
            TEST_COINBASE_NSECS,
            vec![],
            job_id,
        );

        let result = get_distribution(&tracker, 8, Network::Signet, None);

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_get_distribution_multiple_outputs() {
        let tracker = start_tracker_actor();

        let addr1 = parse_address(
            "tb1q3udk7r26qs32ltf9nmqrjaaa7tr55qmkk30q5d",
            Network::Signet,
        )
        .unwrap();
        let addr2 = parse_address(
            "tb1q0afww6y0kgl4tyjjyv6xlttvfwdfqxvrfzz35f",
            Network::Signet,
        )
        .unwrap();

        let outputs = vec![
            TxOut {
                value: Amount::from_str("48 BTC").unwrap(),
                script_pubkey: addr1.script_pubkey(),
            },
            TxOut {
                value: Amount::from_str("2 BTC").unwrap(),
                script_pubkey: addr2.script_pubkey(),
            },
        ];

        let coinbase2 = create_valid_coinbase2(b"P2Poolv2", &outputs);

        let job_id = tracker.get_next_job_id();
        tracker.insert_job(
            Arc::new(create_test_template()),
            "".to_string(),
            coinbase2,
            None,
            TEST_COINBASE_NSECS,
            vec![],
            job_id,
        );

        let result = get_distribution(&tracker, 8, Network::Signet, None);

        assert!(result.is_some());
        let exposition = result.unwrap();
        assert!(exposition.contains(
            "coinbase_output{index=\"0\",address=\"tb1q3udk7r26qs32ltf9nmqrjaaa7tr55qmkk30q5d\"} 4800000000"
        ));
        assert!(exposition.contains(
            "coinbase_output{index=\"1\",address=\"tb1q0afww6y0kgl4tyjjyv6xlttvfwdfqxvrfzz35f\"} 200000000"
        ));
        assert!(exposition.contains("coinbase_total 5000000000"));
    }

    #[tokio::test]
    async fn test_get_distribution_unparseable_script_skips_output() {
        let tracker = start_tracker_actor();

        // Create an output with a valid address and one with an unparseable script
        let addr = parse_address(
            "tb1q3udk7r26qs32ltf9nmqrjaaa7tr55qmkk30q5d",
            Network::Signet,
        )
        .unwrap();

        // OP_RETURN script - can't be converted to an address
        let op_return_script =
            bitcoin::ScriptBuf::from_bytes(vec![0x6a, 0x04, 0xde, 0xad, 0xbe, 0xef]);

        let outputs = vec![
            TxOut {
                value: Amount::from_str("49 BTC").unwrap(),
                script_pubkey: addr.script_pubkey(),
            },
            TxOut {
                value: Amount::ZERO,
                script_pubkey: op_return_script,
            },
        ];

        let coinbase2 = create_valid_coinbase2(b"P2Poolv2", &outputs);

        let job_id = tracker.get_next_job_id();
        tracker.insert_job(
            Arc::new(create_test_template()),
            "".to_string(),
            coinbase2,
            None,
            TEST_COINBASE_NSECS,
            vec![],
            job_id,
        );

        let result = get_distribution(&tracker, 8, Network::Signet, None);

        // Should still return Some, just without the unparseable output
        assert!(result.is_some());
        let exposition = result.unwrap();
        assert!(exposition.contains(
            "coinbase_output{index=\"0\",address=\"tb1q3udk7r26qs32ltf9nmqrjaaa7tr55qmkk30q5d\"} 4900000000"
        ));
        // OP_RETURN output should be skipped (no coinbase_output line for it)
        assert!(!exposition.contains("OP_RETURN"));
        assert!(exposition.contains("coinbase_total 5000000000"));
    }
}
