//! REL-2-06 proof test: emit a `tracing::info!` event through the
//! JSON formatter into an in-memory buffer, then parse the captured
//! bytes as JSON and assert the canonical fields are present.
//!
//! We can't simply call [`init_tracing`] here — `tracing-subscriber`
//! installs a *process-global* default and a second test would race
//! against the first. Instead the test mirrors the formatter
//! construction in `lb_observability::log` and uses
//! `tracing::subscriber::with_default` to make it apply for the
//! scope of a single closure.

use std::io;
use std::sync::{Arc, Mutex};

use lb_observability::LogFormat;
use tracing::subscriber::with_default;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt;

/// Mutex-wrapped `Vec<u8>` that satisfies `io::Write`. Each `make_writer()`
/// hand returns a guarded handle so concurrent writes serialise.
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

/// Find every balanced `{...}` substring in `s` and return them. Naïve
/// brace counter — adequate because the formatter emits one JSON
/// object per event and the only braces inside a line are escaped
/// (inside string literals, which our test inputs avoid).
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

/// Minimal JSON-like field extractor: pulls `"key":"value"` or
/// `"key":<bare-token>`. We deliberately avoid pulling in serde_json
/// to keep dev-deps tight; the formatter's output is well-defined and
/// the test is happy with a regex-grade check.
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

    // Required schema fields — these are the keys operator log
    // shippers grep for, lock them in.
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

    // Level is INFO.
    let level = json_get(obj, "level").unwrap_or("");
    assert_eq!(level, "INFO", "unexpected level in {obj}");

    // Flattened user-supplied `version` field made it through.
    let version = json_get(obj, "version").unwrap_or("");
    assert_eq!(version, "0.1.0", "version field missing/wrong in {obj}");

    // The message field is present (key name is `message` under flatten_event).
    let msg = json_get(obj, "message").unwrap_or("");
    assert!(msg.contains("ExpressGateway"), "message field: {msg}");
}

#[test]
fn test_log_format_env_token_round_trips() {
    // Sanity-check the parser used by init_tracing's LB_LOG_FORMAT
    // resolution. Acts as an in-tree assertion that the operator
    // vocabulary documented in the RUNBOOK stays valid.
    assert_eq!(LogFormat::parse("json"), Some(LogFormat::Json));
    assert_eq!(LogFormat::parse("text"), Some(LogFormat::Text));
    assert_eq!(LogFormat::parse(""), None);
}
