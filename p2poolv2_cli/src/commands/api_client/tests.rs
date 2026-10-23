use super::*;
use p2poolv2_lib::auth::build_basic_auth_header;
use serde::Deserialize;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn make_api_config(port: u16) -> ApiConfig {
    ApiConfig {
        hostname: "127.0.0.1".to_string(),
        port,
        auth_user: None,
        auth_token: None,
        auth_password: None,
        cors_allowed: false,
        public_dashboard: false,
        web_root: None,
    }
}

#[test]
fn test_new_without_auth() {
    let api_config = make_api_config(8080);
    let client = ApiClient::new(&api_config);

    assert_eq!(client.base_url, "http://127.0.0.1:8080");
    assert!(client.auth_header.is_none());
}

#[test]
fn test_new_with_auth() {
    let mut api_config = make_api_config(8080);
    api_config.auth_user = Some("admin".to_string());
    api_config.auth_password = Some("secret".to_string());

    let client = ApiClient::new(&api_config);

    assert_eq!(client.base_url, "http://127.0.0.1:8080");
    assert!(client.auth_header.is_some());

    let expected = build_basic_auth_header("admin", "secret");
    assert_eq!(client.auth_header.unwrap(), expected);
}

#[tokio::test]
async fn test_get_success() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/test"))
        .respond_with(ResponseTemplate::new(200).set_body_string("hello"))
        .expect(1)
        .mount(&mock_server)
        .await;

    let api_config = make_api_config(mock_server.address().port());
    let client = ApiClient::new(&api_config);

    let result = client.get("/test").await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "hello");
}

#[tokio::test]
async fn test_get_sends_auth_header() {
    let mock_server = MockServer::start().await;
    let expected_header = build_basic_auth_header("user", "pass");

    Mock::given(method("GET"))
        .and(path("/secure"))
        .and(header("Authorization", expected_header.as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .expect(1)
        .mount(&mock_server)
        .await;

    let mut api_config = make_api_config(mock_server.address().port());
    api_config.auth_user = Some("user".to_string());
    api_config.auth_password = Some("pass".to_string());

    let client = ApiClient::new(&api_config);
    let result = client.get("/secure").await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_get_returns_error_on_non_success_status() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/fail"))
        .respond_with(ResponseTemplate::new(500).set_body_string("Internal Error"))
        .expect(1)
        .mount(&mock_server)
        .await;

    let api_config = make_api_config(mock_server.address().port());
    let client = ApiClient::new(&api_config);

    let result = client.get("/fail").await;
    assert!(result.is_err());
    let error_message = result.unwrap_err().to_string();
    assert!(error_message.contains("500"));
}

#[tokio::test]
async fn test_get_returns_error_when_server_unreachable() {
    let api_config = make_api_config(19998);
    let client = ApiClient::new(&api_config);

    let result = client.get("/unreachable").await;
    assert!(result.is_err());
    let error_message = result.unwrap_err().to_string();
    assert!(error_message.contains("Is the node running?"));
}

#[derive(Deserialize, Debug, PartialEq)]
struct TestPayload {
    value: i32,
}

#[tokio::test]
async fn test_get_json_success() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/json"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(r#"{"value":42}"#, "application/json"),
        )
        .expect(1)
        .mount(&mock_server)
        .await;

    let api_config = make_api_config(mock_server.address().port());
    let client = ApiClient::new(&api_config);

    let result: Result<TestPayload, _> = client.get_json("/json").await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), TestPayload { value: 42 });
}

#[tokio::test]
async fn test_get_json_returns_error_on_invalid_json() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/bad-json"))
        .respond_with(ResponseTemplate::new(200).set_body_raw("not json", "text/plain"))
        .expect(1)
        .mount(&mock_server)
        .await;

    let api_config = make_api_config(mock_server.address().port());
    let client = ApiClient::new(&api_config);

    let result: Result<TestPayload, _> = client.get_json("/bad-json").await;
    assert!(result.is_err());
}
