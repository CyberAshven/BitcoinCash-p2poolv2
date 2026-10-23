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
    // Smoke test: execute should not fail with auto-generated password
    let result = execute("testuser".to_string(), None);
    assert!(result.is_ok());
}

#[test]
fn test_execute_with_custom_password() {
    // Smoke test: execute should not fail with custom password
    let result = execute("testuser".to_string(), Some("mypassword123".to_string()));
    assert!(result.is_ok());
}
