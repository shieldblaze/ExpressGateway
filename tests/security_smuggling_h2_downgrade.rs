use lb_security::*;

#[test]
fn test_smuggling_h2_downgrade_rejected() {
    // H2 request with Connection header -> rejected on downgrade
    let headers = vec![("connection".to_string(), "keep-alive".to_string())];
    assert!(SmuggleDetector::check_h2_downgrade(&headers, true).is_err());

    // Same headers but not from H2 -> OK
    assert!(SmuggleDetector::check_h2_downgrade(&headers, false).is_ok());
}
