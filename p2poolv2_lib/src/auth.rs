// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use base64::Engine;
use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Compute HMAC-SHA256 of password using salt as key, returning the hex-encoded result.
pub fn password_to_hmac(salt: &str, password: &str) -> Result<String, String> {
    let mut mac = HmacSha256::new_from_slice(salt.as_bytes())
        .map_err(|error| format!("Failed to create HMAC: {error}"))?;
    mac.update(password.as_bytes());
    Ok(hex::encode(mac.finalize().into_bytes()))
}

/// Build a Basic auth header value from username and password.
pub fn build_basic_auth_header(username: &str, password: &str) -> String {
    let credentials = format!("{username}:{password}");
    let encoded = base64::engine::general_purpose::STANDARD.encode(credentials.as_bytes());
    format!("Basic {encoded}")
}

#[cfg(test)]
mod tests;
