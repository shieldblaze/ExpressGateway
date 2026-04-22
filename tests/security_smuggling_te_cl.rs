use lb_security::*;

#[test]
fn test_smuggling_te_cl_rejected() {
    // Transfer-Encoding with non-chunked final value -> rejected
    let headers = vec![(
        "transfer-encoding".to_string(),
        "gzip, identity".to_string(),
    )];
    assert!(SmuggleDetector::check_te_cl(&headers).is_err());
}
