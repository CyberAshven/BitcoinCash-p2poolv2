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

/// Write `contents` to a new file only its owner can read.
///
/// Refuses to overwrite an existing file, so earlier credentials are never
/// lost or replaced unnoticed.
fn write_owner_only_file(path: &Path, contents: &str) -> Result<(), Box<dyn Error>> {
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
    file.write_all(contents.as_bytes())?;
    Ok(())
}

/// Execute the gen-auth command.
///
/// No credential reaches the terminal, where it would end up in scrollback
/// and logs: `auth_user` and `auth_token` go to `credentials_file`, ready to
/// copy into `config.toml`, and a generated password to `password_file`.
/// Both are new files only their owner can read. The user already knows a
/// password they gave, so none is written then.
pub fn execute(
    username: String,
    password: Option<String>,
    password_file: &Path,
    credentials_file: &Path,
) -> Result<(), Box<dyn Error>> {
    let (password, generated) = match password {
        None => (generate_password(), true),
        Some(ref p) if p == "-" => (rpassword::prompt_password("Enter password: ")?, false),
        Some(p) => (p, false),
    };
    // Refuse before writing anything, so a run never leaves half its files.
    for path in std::iter::once(credentials_file).chain(generated.then_some(password_file)) {
        if path.exists() {
            return Err(
                format!("{} already exists; it is never overwritten", path.display()).into(),
            );
        }
    }

    let salt = generate_salt();
    let hmac = password_to_hmac(&salt, &password)
        .map_err(|error| format!("Failed to compute HMAC: {error}"))?;

    write_owner_only_file(
        credentials_file,
        &format!(
            "# P2Pool API credentials from p2poolv2_cli gen-auth.\n\
             # Copy these two lines into the [api] section of config.toml.\n\
             auth_user = \"{username}\"\n\
             auth_token = \"{salt}${hmac}\"\n"
        ),
    )?;
    if generated {
        write_owner_only_file(password_file, &format!("{password}\n"))?;
    }

    println!("\n=== API Authentication Credentials ===\n");
    println!(
        "auth_user and auth_token: written to {}",
        credentials_file.display()
    );
    if generated {
        println!("Password: written to {}", password_file.display());
    }
    println!("\nCopy auth_user and auth_token into the [api] section of config.toml.\n");

    Ok(())
}

#[cfg(test)]
mod tests;
