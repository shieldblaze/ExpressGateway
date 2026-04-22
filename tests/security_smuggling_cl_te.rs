use lb_security::*;

#[test]
fn test_smuggling_cl_te_rejected() {
    // Headers with both Content-Length and Transfer-Encoding -> rejected
    let headers = vec![
        ("content-length".to_string(), "10".to_string()),
        ("transfer-encoding".to_string(), "chunked".to_string()),
    ];
    assert!(SmuggleDetector::check_cl_te(&headers).is_err());

    // Headers with only Content-Length -> OK
    let ok_headers = vec![("content-length".to_string(), "10".to_string())];
    assert!(SmuggleDetector::check_cl_te(&ok_headers).is_ok());
}
