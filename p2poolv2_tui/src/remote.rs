// SPDX-FileCopyrightText: 2024 PDM Authors
// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The state of one value read from the node or from BCHN.

/// A value read from the node or BCHN, as the views show it.
///
/// PDM kept an `Option` of the value and an `Option` of the error side by
/// side; one enum makes the four cases explicit.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Remote<T> {
    /// There is nothing to read it from: no node config is loaded.
    #[default]
    Unconfigured,
    /// Asked for, with no answer yet.
    Loading,
    /// The latest answer.
    Ready(T),
    /// The latest request failed with this message.
    Failed(String),
}

impl<T> Remote<T> {
    /// The value, when the latest request succeeded.
    pub fn ready(&self) -> Option<&T> {
        match self {
            Remote::Ready(value) => Some(value),
            Remote::Unconfigured | Remote::Loading | Remote::Failed(_) => None,
        }
    }

    /// The state a finished request leaves.
    pub fn from_result(result: Result<T, String>) -> Self {
        match result {
            Ok(value) => Remote::Ready(value),
            Err(error) => Remote::Failed(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_is_unconfigured() {
        assert_eq!(Remote::<u32>::default(), Remote::Unconfigured);
    }

    #[test]
    fn test_ready_returns_the_value_only_when_ready() {
        assert_eq!(Remote::Ready(7).ready(), Some(&7));
        assert_eq!(Remote::<u32>::Loading.ready(), None);
        assert_eq!(Remote::<u32>::Failed("boom".into()).ready(), None);
    }

    #[test]
    fn test_from_result_maps_both_outcomes() {
        assert_eq!(Remote::from_result(Ok::<u32, String>(3)), Remote::Ready(3));
        assert_eq!(
            Remote::<u32>::from_result(Err("boom".into())),
            Remote::Failed("boom".into())
        );
    }
}
