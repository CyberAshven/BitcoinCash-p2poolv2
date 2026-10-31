// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::stratum::difficulty_adjuster::DifficultyAdjusterTrait;
use crate::stratum::error::Error;
use crate::stratum::merge_disclosure::SV1_MERGE_EXTENSION;
use crate::stratum::messages::{Message, MiningConfigure, Request, Response};
use crate::stratum::session::Session;
use serde_json::{Map, Value, json};
use tracing::debug;

/// Handle the "mining.configure" message.
///
/// `merge_disclosure` says whether this node merge-mines tokens for its
/// miners, so it can disclose their leaves and wins to one that asks for
/// `pickaxe-merge`.
pub async fn handle_configure<'a, D: DifficultyAdjusterTrait>(
    request: Request<'a>,
    session: &mut Session<D>,
    merge_disclosure: bool,
) -> Result<Vec<Message<'a>>, Error> {
    debug!("Handling mining.configure message");
    match request {
        Request::MiningConfigureRequest(configure) => {
            debug!("Received mining.configure request: {:?}", configure);
            handle_mining_configure(configure, session, merge_disclosure)
        }
        _ => Err(Error::InvalidParams(
            "Invalid method for mining.configure".into(),
        )),
    }
}

/// Answer the extensions this node supports: version rolling, and
/// `pickaxe-merge` (`true` where the node merge-mines tokens and so sends
/// the miner its leaves and wins, otherwise `false`). Another extension is
/// left out of the answer; a request naming none this node knows gets no
/// answer, so the miner is not disconnected.
fn handle_mining_configure<'a>(
    request: MiningConfigure,
    session: &mut Session<impl DifficultyAdjusterTrait>,
    merge_disclosure: bool,
) -> Result<Vec<Message<'a>>, Error> {
    let (extensions, configure_params) = request.params.as_ref();
    let mut result = Map::with_capacity(3);
    if configure_params.version_rolling_mask.is_some() {
        result.insert("version-rolling".into(), json!(true));
        result.insert(
            "version-rolling.mask".into(),
            json!(format!("{:x}", session.version_mask)),
        );
    }
    if extensions
        .iter()
        .any(|extension| extension == SV1_MERGE_EXTENSION)
    {
        session.merge_disclosure = merge_disclosure;
        result.insert(SV1_MERGE_EXTENSION.into(), json!(merge_disclosure));
    }
    if result.is_empty() {
        return Ok(vec![]);
    }
    Ok(vec![Message::Response(Response::new_ok(
        Some(request.id),
        Value::Object(result),
    ))])
}

#[cfg(test)]
mod mining_configure_response_tests {
    use p2poolv2_config::DEFAULT_VERSION_MASK;

    use super::*;
    use crate::{
        stratum::difficulty_adjuster::DifficultyAdjuster,
        stratum::messages::{Id, Message, MiningConfigure},
    };

    #[tokio::test]
    async fn test_handle_configure_valid_request() {
        let message = MiningConfigure::new_version_rolling_configure(
            1,
            Some("ffffffff".to_string()),
            None,
            None,
        );

        let mut session =
            Session::<DifficultyAdjuster>::new(1, 1, Some(1000), DEFAULT_VERSION_MASK);

        let result = handle_configure(message, &mut session, false).await;
        assert!(result.is_ok());
        let messages = result.unwrap();
        assert_eq!(messages.len(), 1);

        match &messages[0] {
            Message::Response(response) => {
                assert_eq!(response.id, Some(Id::Number(1)));
                assert!(response.error.is_none());
                assert!(response.result.is_some());
                let result = response.result.as_ref().unwrap();
                assert_eq!(
                    result,
                    &serde_json::json!({
                        "version-rolling": true,
                        "version-rolling.mask": "1fffe000"
                    })
                );
            }
            _ => panic!("Expected a Response message"),
        }
    }

    /// The request a miner sends for version rolling and `pickaxe-merge`.
    fn merge_request() -> Request<'static> {
        serde_json::from_str(
            r#"{"id":1,"method":"mining.configure","params":[["version-rolling","pickaxe-merge"],{"version-rolling.mask":"1fffe000"}]}"#,
        )
        .unwrap()
    }

    /// A node that merge-mines tokens answers `pickaxe-merge` with `true`
    /// beside version rolling, and the session then gets the disclosure.
    #[tokio::test]
    async fn a_merge_mining_node_grants_pickaxe_merge() {
        let mut session =
            Session::<DifficultyAdjuster>::new(1, 1, Some(1000), DEFAULT_VERSION_MASK);

        let messages = handle_configure(merge_request(), &mut session, true)
            .await
            .unwrap();

        let Message::Response(response) = &messages[0] else {
            panic!("Expected a Response message");
        };
        assert_eq!(
            response.result.as_ref().unwrap(),
            &serde_json::json!({
                "version-rolling": true,
                "version-rolling.mask": "1fffe000",
                "pickaxe-merge": true
            })
        );
        assert!(session.merge_disclosure);
    }

    /// A node that merge-mines nothing answers `pickaxe-merge` with
    /// `false`, alone too, and sends nothing new.
    #[tokio::test]
    async fn a_node_without_tokens_answers_pickaxe_merge_with_false() {
        let mut session =
            Session::<DifficultyAdjuster>::new(1, 1, Some(1000), DEFAULT_VERSION_MASK);
        let request: Request = serde_json::from_str(
            r#"{"id":2,"method":"mining.configure","params":[["pickaxe-merge"],{}]}"#,
        )
        .unwrap();

        let messages = handle_configure(request, &mut session, false)
            .await
            .unwrap();

        let Message::Response(response) = &messages[0] else {
            panic!("Expected a Response message");
        };
        assert_eq!(
            response.result.as_ref().unwrap(),
            &serde_json::json!({"pickaxe-merge": false})
        );
        assert!(!session.merge_disclosure);
    }
}
