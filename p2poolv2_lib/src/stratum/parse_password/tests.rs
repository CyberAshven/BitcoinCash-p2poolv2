use super::*;

#[test]
fn test_parse_difficulty_direct() {
    let result = parse_password("d=1000");
    assert_eq!(result.difficulty, Some(1000));
}

#[test]
fn test_parse_difficulty_large_value() {
    let result = parse_password("d=999999");
    assert_eq!(result.difficulty, Some(999999));
}

#[test]
fn test_parse_terahash() {
    let result = parse_password("th=1");
    // 1 TH/s = 10^12 / (0.3 * 2^32) = 776
    assert_eq!(result.difficulty, Some(776));
}

#[test]
fn test_parse_terahash_large() {
    let result = parse_password("th=100");
    let expected = terahash_to_difficulty(100);
    assert_eq!(result.difficulty, Some(expected));
}

#[test]
fn test_difficulty_takes_priority_over_terahash() {
    let result = parse_password("d=500,th=100");
    assert_eq!(result.difficulty, Some(500));
}

#[test]
fn test_difficulty_in_middle_of_string() {
    let result = parse_password("someprefix,d=200,somesuffix");
    assert_eq!(result.difficulty, Some(200));
}

#[test]
fn test_terahash_in_middle_of_string() {
    let result = parse_password("x,th=10,y");
    let expected = terahash_to_difficulty(10);
    assert_eq!(result.difficulty, Some(expected));
}

#[test]
fn test_empty_string_returns_none() {
    let result = parse_password("");
    assert_eq!(result.difficulty, None);
}

#[test]
fn test_no_recognized_option_returns_none() {
    let result = parse_password("x");
    assert_eq!(result.difficulty, None);
}

#[test]
fn test_plain_password_returns_none() {
    let result = parse_password("mypassword123");
    assert_eq!(result.difficulty, None);
}

#[test]
fn test_difficulty_zero_returns_none() {
    let result = parse_password("d=0");
    assert_eq!(result.difficulty, None);
}

#[test]
fn test_terahash_zero_returns_none() {
    let result = parse_password("th=0");
    assert_eq!(result.difficulty, None);
}

#[test]
fn test_case_insensitive_difficulty() {
    let result = parse_password("D=500");
    assert_eq!(result.difficulty, Some(500));
}

#[test]
fn test_case_insensitive_terahash() {
    let result = parse_password("TH=1");
    assert_eq!(result.difficulty, Some(776));
}

#[test]
fn test_space_separated_options() {
    let result = parse_password("password d=300");
    assert_eq!(result.difficulty, Some(300));
}

#[test]
fn test_does_not_match_inside_word() {
    let result = parse_password("method=123");
    assert_eq!(result.difficulty, None);
}

#[test]
fn test_terahash_to_difficulty_conversion() {
    // difficulty = th * 10^12 / (TARGET_DRR * 2^32)
    let divisor = TARGET_DRR * 4_294_967_296.0;

    let diff_1th = (1e12 / divisor) as u64;
    assert_eq!(terahash_to_difficulty(1), diff_1th);

    let diff_10th = (10e12 / divisor) as u64;
    assert_eq!(terahash_to_difficulty(10), diff_10th);

    let diff_1000th = (1000e12 / divisor) as u64;
    assert_eq!(terahash_to_difficulty(1000), diff_1000th);
}
