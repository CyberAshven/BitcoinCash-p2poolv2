use super::*;

#[test]
fn test_new_subscribe() {
    let message = SimpleRequest::new_subscribe(1, "agent".to_string(), "1.0".to_string(), None);
    let serialized_message = serde_json::to_string(&message).unwrap();
    assert_eq!(
        serialized_message,
        r#"{"id":1,"method":"mining.subscribe","params":["agent/1.0"]}"#
    );

    let message = SimpleRequest::new_subscribe(
        42,
        "agent".to_string(),
        "1.0".to_string(),
        Some("extra_nonce".to_string()),
    );

    let serialized_message = serde_json::to_string(&message).unwrap();
    assert_eq!(
        serialized_message,
        r#"{"id":42,"method":"mining.subscribe","params":["agent/1.0","extra_nonce"]}"#
    );
}

#[test]
fn test_new_subscribe_from_luxminer() {
    let message = serde_json::from_str::<Request>(
            r#"{"id":2,"method":"mining.subscribe","params":["Antminer S19k Pro|LUXminer 2025.7.10.152155-6e13fb74|BHB56903|Unknown|Unknown|",null,"test.hydrapool.org:3333",null]}"#,
        ).unwrap();
    let request = match message {
        Request::SimpleRequest(req) => req,
        _ => panic!("Expected SimpleRequest"),
    };

    assert_eq!(request.id, Some(Id::Number(2)), "Expected id to be 2");

    assert_eq!(
        request.method,
        Cow::Borrowed("mining.subscribe"),
        "Expected method to be 'mining.subscribe'"
    );

    assert_eq!(
        request.params[0],
        Some(
            "Antminer S19k Pro|LUXminer 2025.7.10.152155-6e13fb74|BHB56903|Unknown|Unknown|".into()
        )
    );
    assert_eq!(request.params[1], None);
    assert_eq!(request.params[2], Some("test.hydrapool.org:3333".into()));
    assert_eq!(request.params[3], None);
}

#[test]
fn test_new_authorize() {
    let message =
        SimpleRequest::new_authorize(1, "username".to_string(), Some("password".to_string()));

    let serialized_message = serde_json::to_string(&message).unwrap();
    assert_eq!(
        serialized_message,
        r#"{"id":1,"method":"mining.authorize","params":["username","password"]}"#
    );

    let message =
        SimpleRequest::new_authorize(1, "username".to_string(), Some("password".to_string()));

    let serialized_message = serde_json::to_string(&message).unwrap();
    assert_eq!(
        serialized_message,
        r#"{"id":1,"method":"mining.authorize","params":["username","password"]}"#
    );
}

#[test]
fn test_new_submit() {
    let message = SimpleRequest::new_submit(
        1,
        "worker_name".to_string(),
        "job_id".to_string(),
        "extra_nonce2".to_string(),
        "ntime".to_string(),
        "nonce".to_string(),
    );
    let serialized_message = serde_json::to_string(&message).unwrap();
    assert_eq!(
        serialized_message,
        r#"{"id":1,"method":"mining.submit","params":["worker_name","job_id","extra_nonce2","ntime","nonce"]}"#
    );

    let message = SimpleRequest::new_submit(
        5,
        "worker_name".to_string(),
        "job_id".to_string(),
        "extra_nonce2".to_string(),
        "ntime".to_string(),
        "nonce".to_string(),
    );
    let serialized_message = serde_json::to_string(&message).unwrap();
    assert_eq!(
        serialized_message,
        r#"{"id":5,"method":"mining.submit","params":["worker_name","job_id","extra_nonce2","ntime","nonce"]}"#
    );
}

#[test]
fn test_new_notify() {
    let notify_params = NotifyParams {
        job_id: "job_id".to_string(),
        prevhash: "prevhash".to_string(),
        coinbase1: "coinbase1".to_string(),
        coinbase2: "coinbase2".to_string(),
        merkle_branches: vec!["branch1".to_string(), "branch2".to_string()],
        version: "version".to_string(),
        nbits: "nbits".to_string(),
        ntime: "ntime".to_string(),
        clean_jobs: true,
    };

    let message = Notify::new_notify(notify_params);
    let serialized_message = serde_json::to_string(&message).unwrap();
    assert_eq!(
        serialized_message,
        r#"{"method":"mining.notify","params":["job_id","prevhash","coinbase1","coinbase2",["branch1","branch2"],"version","nbits","ntime",true]}"#
    );

    let message = serde_json::from_str::<Notify>(
        r#"{"method":"mining.notify","params":["job_id","prevhash","coinbase1","coinbase2",["branch1","branch2"],"version","nbits","ntime",true]}"#,
    );
    assert!(message.is_ok());
    assert_eq!(
        message.unwrap().params.merkle_branches.len(),
        2,
        "Expected two merkle branches"
    );

    let message = serde_json::from_str::<Notify>(
        r#"{"method":"mining.notify","params":["184678627bb03d4b","00000000044a23dd986cd2562a5bbad49b42e2dd5068adb4aa57e50bff8d2997","00000020010000000000000000000000000000000000000000000000000000000000000000ffffffff25029c0001000401f342680470a418280c","085032506f6f6c7632ffffffff0200f2052a01000000160014274466e754a1c12d0a2d2cc34ceb70d8e017053a0000000000000000266a24aa21a9ede2f61c3f71d1defd3fa999dfa36953755c690689799962b48bebd836974e8cf900000000",[],"20000000","1e0377ae","6842f301",true]}"#,
    );
    assert!(message.is_ok());
    assert_eq!(
        message.unwrap().params.merkle_branches.len(),
        0,
        "Expected empty merkle branches"
    );

    let notify_str =
        include_str!("../../../../p2poolv2_tests/test_data/validation/stratum/a/notify.json");
    let notify: Notify = serde_json::from_str(notify_str).unwrap();
    assert_eq!(notify.method, "mining.notify");
    assert_eq!(
        notify.params.merkle_branches.len(),
        0,
        "Expected empty merkle branches"
    );
}

#[test]
fn test_new_set_difficulty_response() {
    // Test with numeric ID
    let response = Response::new_set_difficulty_response(
        Some(Id::Number(123)),
        500,
        "extranonce_value".to_string(),
        8,
    );
    let serialized = serde_json::to_string(&response).unwrap();
    assert!(serialized.contains(r#""id":123"#));
    assert!(serialized.contains(r#""mining.set_difficulty""#));
    assert!(serialized.contains("500"));
    assert!(serialized.contains(r#""extranonce_value""#));
    assert!(serialized.contains("8"));

    // Test with string ID
    let response = Response::new_set_difficulty_response(
        Some(Id::String("test-id".to_string())),
        1000,
        "nonce42".to_string(),
        4,
    );
    let serialized = serde_json::to_string(&response).unwrap();
    assert!(serialized.contains(r#""id":"test-id""#));
    assert!(serialized.contains("1000"));
    assert!(serialized.contains(r#""nonce42""#));
    assert!(serialized.contains("4"));
}

#[test]
fn test_new_set_difficulty_notification() {
    let message = SetDifficultyNotification::new(1000);
    let serialized_message = serde_json::to_string(&message).unwrap();
    assert_eq!(
        serialized_message,
        r#"{"method":"mining.set_difficulty","params":[1000]}"#
    );
}

#[test]
fn test_error_serialization() {
    let error = Error {
        code: -1,
        message: Cow::Owned("An error occurred".to_string()),
        data: Some(json!("Additional error data")),
    };
    let serialized_error = serde_json::to_string(&error).unwrap();
    assert_eq!(
        serialized_error,
        r#"[-1,"An error occurred","Additional error data"]"#
    );
}

#[test]
fn test_id_serialization_handle_non_numbers() {
    let id_number = Id::Number(42);
    let serialized_id_number = serde_json::to_string(&id_number).unwrap();
    assert_eq!(serialized_id_number, "42");

    let id_string = Id::String("test".to_string());
    let serialized_id_string = serde_json::to_string(&id_string).unwrap();
    assert_eq!(serialized_id_string, r#""test""#);

    let id_none = Id::None(());
    let serialized_id_none = serde_json::to_string(&id_none).unwrap();
    assert_eq!(serialized_id_none, "null");
}

#[test]
fn test_id_variants() {
    // Test number ID
    let json = r#"{"id":123,"method":"test","params":[]}"#;
    let message: SimpleRequest = serde_json::from_str(json).unwrap();
    assert_eq!(message.id, Some(Id::Number(123)));

    // Test string ID
    let json = r#"{"id":"abc","method":"test","params":[]}"#;
    let message: SimpleRequest = serde_json::from_str(json).unwrap();
    assert_eq!(message.id, Some(Id::String("abc".to_string())));

    // Test null ID
    let json = r#"{"id":null,"method":"test","params":[]}"#;
    let message: SimpleRequest = serde_json::from_str(json).unwrap();
    assert_eq!(message.id, None);
}

#[test]
fn test_id_conversions() {
    // From ()
    let id_none = Id::from(());
    assert!(matches!(id_none, Id::None(())));

    // From i64
    let id_number = Id::from(42i64);
    assert!(matches!(id_number, Id::Number(42)));

    // From String
    let id_string = Id::from("test".to_string());
    assert!(matches!(id_string, Id::String(ref s) if s == "test"));

    // PartialEq tests
    assert_eq!(Id::Number(42), Id::Number(42));
    assert_ne!(Id::Number(42), Id::Number(43));
    assert_eq!(
        Id::String("test".to_string()),
        Id::String("test".to_string())
    );
    assert_ne!(
        Id::String("test".to_string()),
        Id::String("other".to_string())
    );
    assert_eq!(Id::None(()), Id::None(()));
    assert_ne!(Id::Number(42), Id::String("42".to_string()));
    assert_ne!(Id::None(()), Id::Number(0));
}

#[test]
fn test_response_creation() {
    // Test new_ok
    let response = Response::new_ok(Some(Id::Number(1)), json!("success"));
    assert_eq!(response.id, Some(Id::Number(1)));
    assert_eq!(response.result, Some(json!("success")));
    assert!(response.error.is_none());

    // Test new_error
    let response = Response::new_error(
        Some(Id::String("abc".to_string())),
        StratumErrorCode::MethodNotFound,
    );
    assert_eq!(response.id, Some(Id::String("abc".to_string())));
    assert!(response.result.is_none());
    assert!(response.error.is_some());
    assert_eq!(response.error.as_ref().unwrap().code, -32601);
    assert_eq!(response.error.as_ref().unwrap().message, "Method not found");

    // Test new_set_difficulty
    let response =
        Response::new_set_difficulty_response(Some(Id::Number(2)), 500, "abc123".to_string(), 4);
    assert_eq!(response.id, Some(Id::Number(2)));
    assert!(response.result.is_some());
    assert!(response.error.is_none());

    if let Some(Value::Array(arr)) = response.result {
        assert_eq!(arr.len(), 4);
        assert_eq!(arr[0], json!("mining.set_difficulty"));
        assert_eq!(arr[1], json!(500));
        assert_eq!(arr[2], json!("abc123"));
        assert_eq!(arr[3], json!(4));
    } else {
        panic!("Expected array result");
    }
}

#[test]
fn test_new_version_rolling_configure() {
    let message =
        MiningConfigure::new_version_rolling_configure(42, Some("00000fff".into()), None, None);
    let serialized_message = serde_json::to_string(&message).unwrap();
    assert_eq!(
        serialized_message,
        r#"{"id":42,"method":"mining.configure","params":[["version-rolling"],{"version-rolling.mask":"00000fff"}]}"#
    );
}
