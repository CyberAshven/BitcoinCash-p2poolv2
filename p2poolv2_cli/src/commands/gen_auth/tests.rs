use super::*;

#[test]
fn test_generate_salt() {
    let salt = generate_salt();
    assert_eq!(salt.len(), 32); // 16 bytes = 32 hex chars
    // Verify it is valid hex
    assert!(hex::decode(&salt).is_ok());
}

#[test]
fn test_generate_password() {
    let password = generate_password();
    // URL-safe base64 encoding of 32 bytes should be around 43 chars
    assert!(password.len() >= 40 && password.len() <= 50);
    // Verify it is valid base64
    assert!(URL_SAFE_NO_PAD.decode(&password).is_ok());
}

/// Reads `auth_user` and the token salt and HMAC back from a credentials file.
fn read_credentials(path: &Path) -> (String, String, String) {
    let text = std::fs::read_to_string(path).unwrap();
    let value = |key: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(key))
            .unwrap()
            .trim_matches('"')
            .to_string()
    };
    let user = value("auth_user = ");
    let token = value("auth_token = ");
    let (salt, hmac) = token.split_once('$').unwrap();
    (user, salt.to_string(), hmac.to_string())
}

#[test]
fn test_execute_with_auto_generated_password() {
    // The generated password and the config lines land in their files, the
    // token matches the password, and a second run refuses to overwrite.
    let dir = tempfile::tempdir().unwrap();
    let password_path = dir.path().join("api-password.txt");
    let credentials_path = dir.path().join("api-credentials.toml");
    execute(
        "testuser".to_string(),
        None,
        &password_path,
        &credentials_path,
    )
    .unwrap();
    let password = std::fs::read_to_string(&password_path).unwrap();
    assert!(URL_SAFE_NO_PAD.decode(password.trim()).is_ok());
    let credentials = std::fs::read_to_string(&credentials_path).unwrap();
    let (user, salt, hmac) = read_credentials(&credentials_path);
    assert_eq!(user, "testuser");
    assert_eq!(password_to_hmac(&salt, password.trim()).unwrap(), hmac);

    assert!(
        execute(
            "testuser".to_string(),
            None,
            &password_path,
            &credentials_path
        )
        .is_err()
    );
    assert_eq!(std::fs::read_to_string(&password_path).unwrap(), password);
    assert_eq!(
        std::fs::read_to_string(&credentials_path).unwrap(),
        credentials
    );
}

#[test]
fn test_execute_with_custom_password() {
    // A supplied password writes the config lines but no password file.
    let dir = tempfile::tempdir().unwrap();
    let password_path = dir.path().join("api-password.txt");
    let credentials_path = dir.path().join("api-credentials.toml");
    execute(
        "testuser".to_string(),
        Some("mypassword123".to_string()),
        &password_path,
        &credentials_path,
    )
    .unwrap();
    assert!(!password_path.exists());
    let (user, salt, hmac) = read_credentials(&credentials_path);
    assert_eq!(user, "testuser");
    assert_eq!(password_to_hmac(&salt, "mypassword123").unwrap(), hmac);
}

#[test]
fn test_execute_refuses_before_writing_anything() {
    // An existing credentials file stops the run before a password file is
    // made, so no generated password is left without its token.
    let dir = tempfile::tempdir().unwrap();
    let password_path = dir.path().join("api-password.txt");
    let credentials_path = dir.path().join("api-credentials.toml");
    std::fs::write(&credentials_path, "kept\n").unwrap();
    assert!(
        execute(
            "testuser".to_string(),
            None,
            &password_path,
            &credentials_path
        )
        .is_err()
    );
    assert!(!password_path.exists());
    assert_eq!(
        std::fs::read_to_string(&credentials_path).unwrap(),
        "kept\n"
    );
}
