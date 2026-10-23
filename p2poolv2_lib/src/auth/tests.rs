use super::*;

#[test]
fn test_password_to_hmac() {
    let salt = "0123456789abcdef0123456789abcdef";
    let password = "testpassword123";
    let hmac = password_to_hmac(salt, password).unwrap();

    // HMAC-SHA256 produces 32 bytes = 64 hex chars
    assert_eq!(hmac.len(), 64);
    // Verify it is valid hex
    assert!(hex::decode(&hmac).is_ok());

    // Verify deterministic (same inputs produce same output)
    let hmac2 = password_to_hmac(salt, password).unwrap();
    assert_eq!(hmac, hmac2);
}

/// Known-answer test pinning the exact HMAC-SHA256 output.
///
/// `password_to_hmac` output is persisted in operator config files as
/// `auth_token`, so a change here silently invalidates every deployed
/// token. The expected value is derived from the HMAC-SHA256 specification
/// independently of this implementation, not captured from it.
#[test]
fn test_password_to_hmac_known_answer() {
    let salt = "0123456789abcdef0123456789abcdef";
    let password = "testpassword123";
    assert_eq!(
        password_to_hmac(salt, password).unwrap(),
        "e6eb13e97ec482011d014112984f4b4e2ff82c25117f07bf4b616257df1923b7"
    );
}

/// Known-answer test for the Basic auth header, which is sent on the wire
/// to bitcoind and to the P2Poolv2 API.
#[test]
fn test_build_basic_auth_header_known_answer() {
    assert_eq!(
        build_basic_auth_header("admin", "secret"),
        "Basic YWRtaW46c2VjcmV0"
    );
}

#[test]
fn test_password_to_hmac_different_inputs() {
    let salt = "0123456789abcdef0123456789abcdef";
    let hmac1 = password_to_hmac(salt, "password1").unwrap();
    let hmac2 = password_to_hmac(salt, "password2").unwrap();

    // Different passwords should produce different HMACs
    assert_ne!(hmac1, hmac2);
}

#[test]
fn test_build_basic_auth_header() {
    let header = build_basic_auth_header("admin", "secret");
    assert!(header.starts_with("Basic "));

    let encoded = &header[6..];
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .unwrap();
    let decoded_str = String::from_utf8(decoded).unwrap();
    assert_eq!(decoded_str, "admin:secret");
}
