// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

/// Custom implementation of Stratum messages with JSON-RPC serialization
use std::borrow::Cow;
use std::vec;

use crate::stratum::error::StratumErrorCode;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// JSON-RPC ID can be a number, string, or null.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Id {
    Number(u64),
    String(String),
    None(()),
}

impl From<()> for Id {
    fn from(_val: ()) -> Self {
        Id::None(())
    }
}

impl From<i64> for Id {
    fn from(val: i64) -> Self {
        Id::Number(val as u64)
    }
}

impl From<String> for Id {
    fn from(val: String) -> Self {
        Id::String(val)
    }
}

impl PartialEq for Id {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Id::Number(a), Id::Number(b)) => a == b,
            (Id::String(a), Id::String(b)) => a == b,
            (Id::None(_), Id::None(_)) => true,
            _ => false,
        }
    }
}

/// StratumError represents the error structure in Stratum responses
#[derive(Debug, Clone, Deserialize)]
pub struct Error<'a> {
    pub code: i32,
    #[serde(borrow)]
    pub message: Cow<'a, str>,
    pub data: Option<Value>,
}

// Serialize as the standard 3-element array: [code, message, null]
impl Serialize for Error<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeSeq;
        let mut seq = serializer.serialize_seq(Some(3))?;
        seq.serialize_element(&self.code)?;
        seq.serialize_element(&self.message)?;
        seq.serialize_element(&self.data.clone().unwrap_or(json!("")))?;
        seq.end()
    }
}

/// Message type capturing all possible stratum message types.
/// This allows our message handlers to return any of the types and be able to send updates to clients as required.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum Message<'a> {
    Request(Request<'a>),
    Response(Response<'a>),
    Notify(Notify),
    SetDifficulty(SetDifficultyNotification<'a>),
    Extension(ExtensionNotification),
}

/// A notification of a stratum extension the miner asked for in
/// `mining.configure`, such as Pickaxe's `pickaxe.merge_leaves`:
/// `{"id":null,"method":...,"params":[...]}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtensionNotification {
    pub id: Id,
    pub method: String,
    pub params: Vec<Value>,
}

impl ExtensionNotification {
    /// The notification `method` with `params` and a null id.
    pub fn new(method: &str, params: Vec<Value>) -> Self {
        Self {
            id: Id::None(()),
            method: method.to_string(),
            params,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged, bound(deserialize = "'de: 'a"))]
pub enum Request<'a> {
    SimpleRequest(SimpleRequest<'a>),
    MiningConfigureRequest(MiningConfigure<'a>),
    SuggestDifficultyRequest(SuggestDifficulty<'a>),
}

/// Request represents a Stratum request message from client to the server
/// The params in this message are all strings
///
/// We are not using enum to capture the different request types, as we want to remain flexible
/// for handling the diverse Stratum client implementations out there.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimpleRequest<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    #[serde(borrow)]
    pub method: Cow<'a, str>,
    #[serde(borrow, default)]
    pub params: Cow<'a, Vec<Option<String>>>,
}

/// Suggested difficulty request uses a vector of integers of size one as params
/// This is different from SimpleRequest using a vector of string.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuggestDifficulty<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    #[serde(borrow)]
    pub method: Cow<'a, str>,
    #[serde(borrow, default)]
    pub params: Cow<'a, Vec<u64>>,
}

/// Struct for mining.configure messages
/// These messages have mixed types in params and those are defined in MiningConfigureParams.
///
/// mining.configure params example:
/// params: [["minimum-difficulty", "version-rolling"], {<min diff object>, {version rolling object}}]
/// Ref: https://github.com/bitcoin/bips/blob/master/bip-0310.mediawiki
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct MiningConfigure<'a> {
    pub id: Id,
    #[serde(borrow)]
    pub method: Cow<'a, String>,
    #[serde(borrow)]
    pub params: Cow<'a, (Vec<String>, MiningConfigureParams<'a>)>,
}

/// The object part of the mining.configure messages
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct MiningConfigureParams<'a> {
    #[serde(
        rename = "minimum-difficulty.value",
        skip_serializing_if = "Option::is_none"
    )]
    pub minimum_difficulty_value: Option<u32>,

    #[serde(
        rename = "version-rolling.mask",
        skip_serializing_if = "Option::is_none",
        borrow
    )]
    pub version_rolling_mask: Cow<'a, Option<String>>,

    #[serde(
        rename = "version-rolling.min-bit-count",
        skip_serializing_if = "Option::is_none"
    )]
    pub version_rolling_min_bit_count: Option<u32>,
}

/// Response represents a Stratum response message from the server to the client
/// We use Value in result to allow for different types of responses.
/// TODO: Consider using various Response types to avoiding using Value (which will result in memory allocations)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>, // Should match the id from the request
    pub result: Option<Value>,
    #[serde(borrow)]
    pub error: Option<Error<'a>>,
}

/// The mining.notify message is used to notify the client of new work
/// The params is an array of strings and one element is a nested array of strings (the merkle branches)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notify {
    pub method: String,
    pub params: NotifyParams,
}

/// mining.set_difficulty message is used to notify the client of a change in difficulty
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetDifficultyNotification<'a> {
    #[serde(borrow)]
    pub method: Cow<'a, str>,
    pub params: Vec<u64>,
}

/// NotifyParams represents the parameters for the mining.notify message
/// It includes job_id, prevhash, coinbase1, coinbase2, merkle_branches,
/// version, nbits, ntime, and clean_jobs
#[derive(Debug, Clone)]
pub struct NotifyParams {
    pub job_id: String,
    pub prevhash: String,
    pub coinbase1: String,
    pub coinbase2: String,
    pub merkle_branches: Vec<String>,
    pub version: String,
    pub nbits: String,
    pub ntime: String,
    pub clean_jobs: bool,
}

// Custom serializer to output an array format instead of keyed object
impl Serialize for NotifyParams {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeSeq;

        let mut seq = serializer.serialize_seq(Some(9))?;
        seq.serialize_element(&self.job_id)?;
        seq.serialize_element(&self.prevhash)?;
        seq.serialize_element(&self.coinbase1)?;
        seq.serialize_element(&self.coinbase2)?;
        seq.serialize_element(&self.merkle_branches)?;
        seq.serialize_element(&self.version)?;
        seq.serialize_element(&self.nbits)?;
        seq.serialize_element(&self.ntime)?;
        seq.serialize_element(&self.clean_jobs)?;
        seq.end()
    }
}

// Custom deserializer to parse the array format into NotifyParams
impl<'de> Deserialize<'de> for NotifyParams {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let vec: Vec<Value> = Vec::deserialize(deserializer)?;
        if vec.len() != 9 {
            return Err(serde::de::Error::custom("Invalid number of fields"));
        }

        Ok(NotifyParams {
            job_id: vec[0].as_str().unwrap_or_default().to_string(),
            prevhash: vec[1].as_str().unwrap_or_default().to_string(),
            coinbase1: vec[2].as_str().unwrap_or_default().to_string(),
            coinbase2: vec[3].as_str().unwrap_or_default().to_string(),
            merkle_branches: vec
                .get(4)
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .map(|v| v.as_str().unwrap_or_default().to_string())
                        .collect()
                })
                .unwrap_or_default(),
            version: vec[5].as_str().unwrap_or_default().to_string(),
            nbits: vec[6].as_str().unwrap_or_default().to_string(),
            ntime: vec[7].as_str().unwrap_or_default().to_string(),
            clean_jobs: vec[8].as_bool().unwrap_or(false),
        })
    }
}

/// Request represents a Stratum request message from client to the server
/// It includes the id, method, and params
/// The methods supported are mining.subscribe, mining.authorize, and mining.submit
impl SimpleRequest<'_> {
    /// Creates a new subscribe message with an optional id and params
    /// If no params are provided, it defaults to an empty array
    /// If no id is provided, it defaults to None
    /// The user agent and version are concatenated with a slash
    pub fn new_subscribe(
        id: u64,
        user_agent: String,
        version: String,
        extra_nonce: Option<String>,
    ) -> Self {
        let user_agent_param = user_agent + "/" + &version;
        let mut params = vec![(Some(user_agent_param))];
        if extra_nonce.is_some() {
            params.push(extra_nonce);
        }
        SimpleRequest {
            id: Some(Id::Number(id)),
            method: Cow::Owned("mining.subscribe".to_string()),
            params: Cow::Owned(params),
        }
    }

    /// Creates a new authorize message
    /// If no id is provided, it defaults to None
    /// The username and password are passed as parameters
    pub fn new_authorize(id: u64, username: String, password: Option<String>) -> Self {
        let mut params = vec![(Some(username))];
        if let Some(password) = password {
            params.push(Some(password));
        }
        SimpleRequest {
            id: Some(Id::Number(id)),
            method: Cow::Owned("mining.authorize".to_string()),
            params: Cow::Owned(params),
        }
    }

    /// Creates a new submit message
    /// The server never creates this message, but it is used by the client to submit work
    pub fn new_submit(
        id: u64,
        username: String,
        job_id: String,
        extra_nonce2: String,
        n_time: String,
        nonce: String,
    ) -> Self {
        let params = vec![
            Some(username),
            Some(job_id),
            Some(extra_nonce2),
            Some(n_time),
            Some(nonce),
        ];
        SimpleRequest {
            id: Some(Id::Number(id)),
            method: Cow::Owned("mining.submit".to_string()),
            params: Cow::Owned(params),
        }
    }
}

impl<'a> MiningConfigure<'a> {
    /// Create a new mining.configure message for version-rolling
    pub fn new_version_rolling_configure(
        id: u64,
        version_rolling_mask: Option<String>,
        version_rolling_min_bit_count: Option<u32>,
        minimum_difficulty_value: Option<u32>,
    ) -> Request<'a> {
        let params = (
            vec!["version-rolling".to_string()],
            MiningConfigureParams {
                minimum_difficulty_value,
                version_rolling_mask: Cow::Owned(version_rolling_mask),
                version_rolling_min_bit_count,
            },
        );

        Request::MiningConfigureRequest(MiningConfigure {
            id: Id::Number(id),
            method: Cow::Owned("mining.configure".to_string()),
            params: Cow::Owned(params),
        })
    }
}

/// Response represents a Stratum response message from the server to the client
/// Supported responses are set_difficulty (in response to subscribe), ok and error.
impl Response<'_> {
    pub fn new_set_difficulty_response(
        id: Option<Id>,
        difficulty: u64,
        extra_nonce: String,
        extra_nonce_size: u8,
    ) -> Self {
        let response_details = vec![
            json!("mining.set_difficulty"),
            json!(difficulty),
            json!(extra_nonce),
            json!(extra_nonce_size),
        ];
        Response {
            id,
            result: Some(Value::Array(response_details)),
            error: None,
        }
    }

    pub fn new_ok(id: Option<Id>, result: Value) -> Self {
        Response {
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn new_error(id: Option<Id>, error_code: StratumErrorCode) -> Self {
        Response {
            id,
            result: None,
            error: Some(Error {
                // SAFETY: [StratumErrorCode] is repr(i32)
                code: error_code as i32,
                message: Cow::Owned(error_code.to_string()),
                data: None,
            }),
        }
    }

    /// Override the default error message with a custom one.
    pub fn with_message(mut self, msg: String) -> Self {
        if let Some(ref mut error) = self.error {
            error.message = Cow::Owned(msg);
        }
        self
    }
}

/// Notify represents a Stratum notification message from the server to the client
/// It is used to notify the client of new work and changes in difficulty
impl Notify {
    /// Creates a new notify message with the given parameters
    pub fn new_notify(params: NotifyParams) -> Self {
        Notify {
            method: "mining.notify".to_string(),
            params,
        }
    }
}

impl SetDifficultyNotification<'_> {
    /// Creates a new set_difficulty notification with the given parameters
    pub fn new(difficulty: u64) -> Self {
        SetDifficultyNotification {
            method: Cow::Borrowed("mining.set_difficulty"),
            params: vec![difficulty],
        }
    }
}

#[cfg(test)]
mod tests;
