// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Bitcoin Cash networks as BCHN, the node config and the node's API name
//! them.
//!
//! Mainnet and chipnet run the same code: the network is a value read from
//! the node config, BCHN and `/network`, and the views only compare the
//! three.

/// A Bitcoin Cash network.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CashNetwork {
    Mainnet,
    Chipnet,
    Testnet3,
    Testnet4,
    Scalenet,
    Regtest,
}

impl CashNetwork {
    /// The network BCHN reports in `getblockchaininfo`'s `chain`: `main`,
    /// `chip`, `test`, `test4`, `scale` or `regtest`.
    pub fn from_bchn_chain(chain: &str) -> Option<Self> {
        match chain {
            "main" => Some(CashNetwork::Mainnet),
            "chip" => Some(CashNetwork::Chipnet),
            "test" => Some(CashNetwork::Testnet3),
            "test4" => Some(CashNetwork::Testnet4),
            "scale" => Some(CashNetwork::Scalenet),
            "regtest" => Some(CashNetwork::Regtest),
            _ => None,
        }
    }

    /// The network in `/network`'s `badge`: `mainnet`, `chipnet` or
    /// `regtest`. A Bitcoin node's badge, `bitcoin`, is none of them.
    pub fn from_badge(badge: &str) -> Option<Self> {
        match badge {
            "mainnet" => Some(CashNetwork::Mainnet),
            "chipnet" => Some(CashNetwork::Chipnet),
            "regtest" => Some(CashNetwork::Regtest),
            _ => None,
        }
    }

    /// The name shown on screen.
    pub fn name(self) -> &'static str {
        match self {
            CashNetwork::Mainnet => "mainnet",
            CashNetwork::Chipnet => "chipnet",
            CashNetwork::Testnet3 => "testnet3",
            CashNetwork::Testnet4 => "testnet4",
            CashNetwork::Scalenet => "scalenet",
            CashNetwork::Regtest => "regtest",
        }
    }
}

/// What to warn about when a source reports another network than the node
/// config, or `None` when they agree or either is unknown.
///
/// `found` is the network the source reported, `None` when its name is
/// none this tool knows; `found_name` is that name as the source wrote it,
/// shown when `found` is `None`.
pub fn mismatch(
    expected: Option<CashNetwork>,
    source: &str,
    found: Option<CashNetwork>,
    found_name: &str,
) -> Option<String> {
    let expected = expected?;
    if found == Some(expected) {
        return None;
    }
    let shown = found.map_or(found_name, |network| network.name());
    Some(format!(
        "{source} is on {shown} but the node config says {}",
        expected.name()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_bchn_chain_reads_chipnet() {
        assert_eq!(
            CashNetwork::from_bchn_chain("chip"),
            Some(CashNetwork::Chipnet)
        );
    }

    #[test]
    fn test_from_bchn_chain_reads_mainnet() {
        assert_eq!(
            CashNetwork::from_bchn_chain("main"),
            Some(CashNetwork::Mainnet)
        );
    }

    #[test]
    fn test_from_bchn_chain_reads_the_other_test_networks() {
        assert_eq!(
            CashNetwork::from_bchn_chain("test4"),
            Some(CashNetwork::Testnet4)
        );
        assert_eq!(
            CashNetwork::from_bchn_chain("scale"),
            Some(CashNetwork::Scalenet)
        );
        assert_eq!(
            CashNetwork::from_bchn_chain("test"),
            Some(CashNetwork::Testnet3)
        );
        assert_eq!(
            CashNetwork::from_bchn_chain("regtest"),
            Some(CashNetwork::Regtest)
        );
    }

    #[test]
    fn test_from_bchn_chain_rejects_unknown_names() {
        assert_eq!(CashNetwork::from_bchn_chain("signet"), None);
    }

    #[test]
    fn test_from_badge_reads_the_api_badges() {
        assert_eq!(
            CashNetwork::from_badge("chipnet"),
            Some(CashNetwork::Chipnet)
        );
        assert_eq!(
            CashNetwork::from_badge("mainnet"),
            Some(CashNetwork::Mainnet)
        );
        assert_eq!(CashNetwork::from_badge("bitcoin"), None);
    }

    #[test]
    fn test_mismatch_is_none_when_the_source_agrees() {
        assert_eq!(
            mismatch(
                Some(CashNetwork::Chipnet),
                "BCHN",
                Some(CashNetwork::Chipnet),
                "chip"
            ),
            None
        );
    }

    #[test]
    fn test_mismatch_names_both_networks() {
        assert_eq!(
            mismatch(
                Some(CashNetwork::Chipnet),
                "BCHN",
                Some(CashNetwork::Testnet4),
                "test4"
            )
            .as_deref(),
            Some("BCHN is on testnet4 but the node config says chipnet")
        );
    }

    #[test]
    fn test_mismatch_keeps_an_unknown_name() {
        assert_eq!(
            mismatch(Some(CashNetwork::Mainnet), "P2Pool API", None, "bitcoin").as_deref(),
            Some("P2Pool API is on bitcoin but the node config says mainnet")
        );
    }

    #[test]
    fn test_mismatch_is_none_without_an_expected_network() {
        assert_eq!(
            mismatch(None, "BCHN", Some(CashNetwork::Mainnet), "main"),
            None
        );
    }
}
