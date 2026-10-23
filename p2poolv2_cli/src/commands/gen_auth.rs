// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use p2poolv2_lib::auth::password_to_hmac;
use rand::RngExt;
use std::error::Error;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

/// Generate a 16-byte hex salt (32 hex characters)
fn generate_salt() -> String {
    let mut rng = rand::rng();
    let salt_bytes: [u8; 16] = rng.random();
    hex::encode(salt_bytes)
}

/// Generate a 32-byte URL-safe base64 password
fn generate_password() -> String {
    let mut rng = rand::rng();
    let password_bytes: [u8; 32] = rng.random();
    URL_SAFE_NO_PAD.encode(password_bytes)
}

/// Write a generated password to a new file only its owner can read.
///
/// Refuses to overwrite an existing file, so an earlier password is never
/// lost or replaced unnoticed.
fn write_password_file(path: &Path, password: &str) -> Result<(), Box<dyn Error>> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|error| format!("Could not create {}: {error}", path.display()))?;
    writeln!(file, "{password}")?;
    Ok(())
}

/// Execute the gen-auth command
pub fn execute(
    username: String,
    password: Option<String>,
    password_file: &Path,
) -> Result<(), Box<dyn Error>> {
    // Determine password (generate, prompt, or use provided). A generated
    // password goes to `password_file`, never to the terminal; the user
    // already knows one they gave.
    let (password, generated) = match password {
        None => (generate_password(), true),
        Some(ref p) if p == "-" => (rpassword::prompt_password("Enter password: ")?, false),
        Some(p) => (p, false),
    };

    // Generate salt
    let salt = generate_salt();

    // Compute HMAC
    let hmac = password_to_hmac(&salt, &password)
        .map_err(|error| format!("Failed to compute HMAC: {error}"))?;

    // Display results in an easy-to-copy format
    println!("\n=== API Authentication Credentials ===\n");
    println!("Username: {username}");
    if generated {
        write_password_file(password_file, &password)?;
        println!("Password: written to {}", password_file.display());
    }
    println!("Salt:     {salt}");
    println!("HMAC:     {hmac}");
    println!("\n=== Add to config.toml ===\n");
    println!("auth_user = \"{username}\"");
    println!("auth_token = \"{salt}${hmac}\"");
    println!();

    Ok(())
}

#[cfg(test)]
mod tests;
