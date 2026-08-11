//! JSON log-schema proof. `init_tracing` CANNOT be used here — it installs a process-global default
//! and a second test would race it — so the formatter construction is mirrored under `with_default`.
//! Keep the mirror in sync with `lb_observability::log`.

use std::io;
use std::sync::{Arc, Mutex};

use lb_observability::LogFormat;
use tracing::subscriber::with_default;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt;

#[derive(Clone, Default)]
struct CaptureWriter {
    buf: Arc<Mutex<Vec<u8>>>,
}

impl CaptureWriter {
    fn snapshot(&self) -> Vec<u8> {
        self.buf.lock().expect("poisoned").clone()
    }
}

impl io::Write for CaptureWriter {
    fn write(&mut self, src: &[u8]) -> io::Result<usize> {
        self.buf.lock().expect("poisoned").extend_from_slice(src);
        Ok(src.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CaptureWriter {
    type Writer = CaptureWriter;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// Naïve balanced-brace split; adequate only because the test inputs contain no braces in
/// string literals.
fn json_objects(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    let mut start = None;
    let mut depth: i32 = 0;
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'{' => {
                if depth == 0 {
                    start = Some(i);
                }
                depth += 1;
            }
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    if let Some(s0) = start.take() {
                        out.push(&s[s0..=i]);
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Regex-grade field extractor, avoiding a serde_json dev-dep.
fn json_get<'a>(blob: &'a str, key: &str) -> Option<&'a str> {
    let needle = format!("\"{key}\":");
    let after = blob.find(&needle).map(|i| i + needle.len())?;
    let rest = &blob[after..];
    let rest = rest.trim_start();
    if let Some(stripped) = rest.strip_prefix('"') {
        let end = stripped.find('"')?;
        Some(&stripped[..end])
    } else {
        let end = rest.find([',', '}', '\n']).unwrap_or(rest.len());
        Some(rest[..end].trim())
    }
}

#[test]
fn test_json_log_emits_json() {
    let writer = CaptureWriter::default();
    let subscriber = fmt()
        .json()
        .flatten_event(true)
        .with_current_span(false)
        .with_span_list(false)
        .with_target(true)
        .with_writer(writer.clone())
        .with_env_filter(EnvFilter::new("info"))
        .finish();

    with_default(subscriber, || {
        tracing::info!(version = "0.1.0", "ExpressGateway test");
    });

    let bytes = writer.snapshot();
    let text = String::from_utf8(bytes).expect("utf-8 output");
    assert!(!text.is_empty(), "subscriber emitted no output");

    let objects = json_objects(&text);
    assert!(
        !objects.is_empty(),
        "no JSON object found in output:\n{text}",
    );
    let obj = objects[0];

    // Log shippers grep for these keys.
    assert!(
        obj.contains("\"timestamp\":"),
        "JSON object missing timestamp: {obj}",
    );
    assert!(
        obj.contains("\"level\":"),
        "JSON object missing level: {obj}",
    );
    assert!(
        obj.contains("\"target\":"),
        "JSON object missing target: {obj}",
    );

    let level = json_get(obj, "level").unwrap_or("");
    assert_eq!(level, "INFO", "unexpected level in {obj}");

    let version = json_get(obj, "version").unwrap_or("");
    assert_eq!(version, "0.1.0", "version field missing/wrong in {obj}");

    let msg = json_get(obj, "message").unwrap_or("");
    assert!(msg.contains("ExpressGateway"), "message field: {msg}");
}

#[test]
fn test_log_format_env_token_round_trips() {
    // Keeps the documented `LB_LOG_FORMAT` vocabulary valid.
    assert_eq!(LogFormat::parse("json"), Some(LogFormat::Json));
    assert_eq!(LogFormat::parse("text"), Some(LogFormat::Text));
    assert_eq!(LogFormat::parse(""), None);
}
