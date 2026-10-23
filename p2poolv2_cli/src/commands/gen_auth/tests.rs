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

#[test]
fn test_execute_with_auto_generated_password() {
    // The generated password lands in the file, and a second run refuses
    // to overwrite it.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("api-password.txt");
    execute("testuser".to_string(), None, &path).unwrap();
    let written = std::fs::read_to_string(&path).unwrap();
    assert!(URL_SAFE_NO_PAD.decode(written.trim()).is_ok());
    assert!(execute("testuser".to_string(), None, &path).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), written);
}

#[test]
fn test_execute_with_custom_password() {
    // Smoke test: execute should not fail with custom password
    // A supplied password writes no file.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("api-password.txt");
    let result = execute(
        "testuser".to_string(),
        Some("mypassword123".to_string()),
        &path,
    );
    assert!(result.is_ok());
    assert!(!path.exists());
}
