// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Learn the Bitcoin Cash headers a peer's share message carries
//! (`shares::bch_carried_headers`), before its shares are handled.

use crate::shares::bch_carried_headers::accept_carried;
#[cfg(test)]
#[mockall_double::double]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
#[cfg(not(test))]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
use crate::shares::validation::ShareValidator;
use bitcoin::block::Header;
use std::error::Error;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::debug;

/// Check `headers` and store those that pass. A header that fails its check
/// is an error for the message: the peer sent a header that cost no real
/// proof of work, or broke Bitcoin Cash's difficulty rules. Headers whose
/// parent this node does not know yet are left for a later message. A
/// Bitcoin node ignores carried headers.
pub(crate) async fn learn_carried_bch_headers(
    headers: &[Header],
    chain_store_handle: &ChainStoreHandle,
    share_validator: &(dyn ShareValidator + Send + Sync),
) -> Result<(), Box<dyn Error + Send + Sync>> {
    if headers.is_empty() {
        return Ok(());
    }
    let Some(rules) = share_validator.bch_header_rules() else {
        return Ok(());
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u32::try_from(elapsed.as_secs()).unwrap_or(u32::MAX)
        });
    let outcome = accept_carried(
        headers,
        |hash| chain_store_handle.get_bch_header(hash).ok().flatten(),
        &rules,
        now,
    );
    if !outcome.unlinked.is_empty() {
        debug!(
            "{} carried Bitcoin Cash headers do not link to a known header yet",
            outcome.unlinked.len()
        );
    }
    if !outcome.accepted.is_empty() {
        chain_store_handle.add_bch_headers(outcome.accepted).await?;
    }
    match outcome.failure {
        Some(failure) => {
            Err(format!("peer sent a Bitcoin Cash header that fails its check: {failure}").into())
        }
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::BitcoinCashNetwork;
    use crate::shares::bch_asert::tests::{MAINNET_PAIRS, header};
    use crate::shares::bch_headers::{BchHeaderRules, KnownBchHeader};
    use crate::shares::validation::MockDefaultShareValidator;

    fn first() -> KnownBchHeader {
        KnownBchHeader {
            height: MAINNET_PAIRS[0].0,
            header: header(MAINNET_PAIRS[0].1),
        }
    }

    fn mainnet_validator() -> MockDefaultShareValidator {
        let mut validator = MockDefaultShareValidator::default();
        validator
            .expect_bch_header_rules()
            .return_const(Some(BchHeaderRules::for_network(
                BitcoinCashNetwork::Mainnet,
            )));
        validator
    }

    fn store_knowing_first() -> ChainStoreHandle {
        let mut store = ChainStoreHandle::default();
        let known = first();
        store
            .expect_get_bch_header()
            .returning(move |hash| Ok((known.block_hash() == *hash).then_some(known)));
        store
    }

    #[tokio::test]
    async fn a_header_on_a_known_parent_is_stored() {
        let second = header(MAINNET_PAIRS[1].1);
        let mut store = store_knowing_first();
        store
            .expect_add_bch_headers()
            .withf(move |headers| headers.len() == 1 && headers[0].header == second)
            .times(1)
            .returning(|_| Ok(()));

        learn_carried_bch_headers(&[second], &store, &mainnet_validator())
            .await
            .unwrap();
    }

    /// A header without its proof of work is an error for the message, and
    /// nothing is stored.
    #[tokio::test]
    async fn a_header_failing_its_check_fails_the_message() {
        let mut forged = header(MAINNET_PAIRS[1].1);
        forged.nonce = forged.nonce.wrapping_add(1);
        let mut store = store_knowing_first();
        store.expect_add_bch_headers().never();

        let error = learn_carried_bch_headers(&[forged], &store, &mainnet_validator())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("fails its check"), "{error}");
    }

    /// A Bitcoin node has no BCH header rules and ignores carried headers.
    #[tokio::test]
    async fn a_bitcoin_node_ignores_carried_headers() {
        let mut validator = MockDefaultShareValidator::default();
        validator.expect_bch_header_rules().return_const(None);
        let store = ChainStoreHandle::default();

        learn_carried_bch_headers(&[header(MAINNET_PAIRS[1].1)], &store, &validator)
            .await
            .unwrap();
    }
}
