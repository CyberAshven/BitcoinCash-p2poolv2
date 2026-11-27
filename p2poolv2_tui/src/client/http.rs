// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What the API and BCHN clients share: the HTTP client, bounded body
//! reads and error messages without URLs.

use reqwest::{Client, Response};
use std::error::Error;
use std::time::Duration;

/// How long one request may take.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// The largest body read from one answer.
pub const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;

/// Most causes listed in an error message.
const MAX_CAUSES: usize = 4;

/// An HTTP client with [`REQUEST_TIMEOUT`].
pub fn build_client() -> Result<Client, String> {
    Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|error| format!("Failed to create the HTTP client: {}", describe(error)))
}

/// A request error with its causes, e.g. `error sending request: client
/// error (Connect): tcp connect error: ...`, without the URL, which can
/// carry a login.
pub fn describe(error: reqwest::Error) -> String {
    let error = error.without_url();
    let mut message = error.to_string();
    let mut cause = error.source();
    let mut listed = 0;
    while let Some(source) = cause
        && listed < MAX_CAUSES
    {
        message.push_str(": ");
        message.push_str(&source.to_string());
        cause = source.source();
        listed += 1;
    }
    message
}

/// The body of `response`, refusing one longer than `limit` bytes before
/// reading it, when the length is announced, or as soon as it grows past.
pub async fn read_capped(mut response: Response, limit: usize) -> Result<Vec<u8>, String> {
    let too_long = || format!("the answer is longer than {limit} bytes");
    let announced = response.content_length().unwrap_or(0);
    if announced > limit as u64 {
        return Err(too_long());
    }
    let mut body = Vec::with_capacity(usize::try_from(announced).unwrap_or(0));
    while let Some(chunk) = response.chunk().await.map_err(describe)? {
        if body.len() + chunk.len() > limit {
            return Err(too_long());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// `url` without a `user:password@` part, to show.
pub fn display_url(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_string();
    };
    let authority_end = rest.find('/').unwrap_or(rest.len());
    match rest[..authority_end].rfind('@') {
        Some(at) => format!("{scheme}://{}", &rest[at + 1..]),
        None => url.to_string(),
    }
}

/// `value` percent-encoded for a URL query, keeping CashAddr's `:`.
pub fn url_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~' | b':') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

#[cfg(test)]
mod tests;
