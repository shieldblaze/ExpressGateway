//! ROUND8-OPS-06 / REL-2-07 proof — the W3C trace-context library now
//! has a production L7 callsite.
//!
//! Reference: REL-2-07 (`audit/reliability/round-2-review.md:341`)
//! shipped `lb_observability::tracing_propagation` in author-sha
//! `1d462c7` and was stuck at `Verified-Fixed-Partial` because NO L7
//! callsite extracted or injected. This test snapshots the request
//! span the H1 proxy now opens and asserts the inbound `trace_id` is
//! carried and the response `http.status_code` is recorded.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lb_io::pool::{PoolConfig, TcpPool};
use lb_io::sockopts::BackendSockOpts;
use lb_l7::h1_proxy::{H1Proxy, HttpTimeouts, RoundRobinAddrs};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tracing::field::{Field, Visit};
use tracing::span::Attributes;
use tracing::{Id, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::SubscriberInitExt;

const CLIENT_TRACEPARENT: &str = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01";

#[derive(Default)]
struct Snapshot {
    /// (span-name, field-name, value-as-string)
    fields: Vec<(String, String, String)>,
}

#[derive(Clone)]
struct CapturingLayer(Arc<Mutex<Snapshot>>);

struct FieldGrab<'a> {
    span: &'a str,
    out: &'a mut Vec<(String, String, String)>,
}

impl Visit for FieldGrab<'_> {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.out.push((
            self.span.to_owned(),
            field.name().to_owned(),
            format!("{value:?}"),
        ));
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        self.out.push((
            self.span.to_owned(),
            field.name().to_owned(),
            value.to_owned(),
        ));
    }
    fn record_u64(&mut self, field: &Field, value: u64) {
        self.out.push((
            self.span.to_owned(),
            field.name().to_owned(),
            value.to_string(),
        ));
    }
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.out.push((
            self.span.to_owned(),
            field.name().to_owned(),
            value.to_string(),
        ));
    }
}

impl<S> Layer<S> for CapturingLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, _id: &Id, _ctx: Context<'_, S>) {
        let name = attrs.metadata().name().to_owned();
        let mut guard = self.0.lock().unwrap();
        let mut grab = FieldGrab {
            span: &name,
            out: &mut guard.fields,
        };
        attrs.record(&mut grab);
    }

    fn on_record(&self, id: &Id, values: &tracing::span::Record<'_>, ctx: Context<'_, S>) {
        let name = ctx
            .span(id)
            .map_or_else(|| "?".to_owned(), |s| s.name().to_owned());
        let mut guard = self.0.lock().unwrap();
        let mut grab = FieldGrab {
            span: &name,
            out: &mut guard.fields,
        };
        values.record(&mut grab);
    }
}

async fn spawn_proxy(backend_addr: SocketAddr) -> SocketAddr {
    let pool = TcpPool::new(
        PoolConfig::default(),
        BackendSockOpts::default(),
        lb_io::Runtime::new(),
    );
    let picker = RoundRobinAddrs::new(vec![backend_addr]).unwrap();
    let proxy = Arc::new(H1Proxy::new(
        pool,
        Arc::new(picker),
        None,
        HttpTimeouts {
            header: Duration::from_secs(2),
            body: Duration::from_secs(2),
            total: Duration::from_secs(5),
        },
        false,
    ));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        if let Ok((sock, peer)) = listener.accept().await {
            let _ = proxy.serve_connection(sock, peer).await;
        }
    });
    addr
}

#[tokio::test]
async fn request_span_carries_trace_id_and_status() {
    let snap = Arc::new(Mutex::new(Snapshot::default()));
    let layer = CapturingLayer(Arc::clone(&snap));
    let subscriber = tracing_subscriber::registry().with(layer);
    let _guard = subscriber.set_default();

    // Backend address points at a closed port — the request fails
    // upstream (502) which is irrelevant: the span + status-record
    // happen regardless of upstream outcome.
    let backend_addr: SocketAddr = "127.0.0.1:1".parse().unwrap();
    let proxy_addr = spawn_proxy(backend_addr).await;

    let mut client = TcpStream::connect(proxy_addr).await.unwrap();
    let req = format!(
        "GET /traced HTTP/1.1\r\nHost: x\r\ntraceparent: {CLIENT_TRACEPARENT}\r\nConnection: close\r\n\r\n"
    );
    client.write_all(req.as_bytes()).await.unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(4), async {
        let mut tmp = [0u8; 1024];
        loop {
            match client.read(&mut tmp).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
    })
    .await;

    // Give the instrumented handler a beat to close the span.
    tokio::time::sleep(Duration::from_millis(100)).await;

    let g = snap.lock().unwrap();
    let span_fields: Vec<_> = g
        .fields
        .iter()
        .filter(|(s, _, _)| s == "lb.l7.request")
        .collect();
    assert!(
        !span_fields.is_empty(),
        "expected a lb.l7.request span to be emitted; captured: {:?}",
        g.fields
    );
    let trace_id = span_fields
        .iter()
        .find(|(_, f, _)| f == "trace_id")
        .map(|(_, _, v)| v.clone())
        .expect("span must carry trace_id");
    assert!(
        trace_id.contains("0af7651916cd43dd8448eb211c80319c"),
        "span trace_id must equal the client's inbound trace-id: {trace_id:?}"
    );
    let parent_id = span_fields
        .iter()
        .find(|(_, f, _)| f == "parent_id")
        .map(|(_, _, v)| v.clone())
        .expect("span must carry parent_id");
    assert!(
        parent_id.contains("b7ad6b7169203331"),
        "parent_id field records the inbound parent for correlation: {parent_id:?}"
    );
    // The response status was recorded onto the span (proves the
    // `span.record("http.status_code", ...)` on the response path
    // runs). Upstream is a closed port so the status is 502.
    let status = span_fields
        .iter()
        .find(|(_, f, _)| f == "http.status_code")
        .map(|(_, _, v)| v.clone());
    assert!(
        status.is_some_and(|v| v.contains("502")),
        "response status must be recorded on the span; captured: {:?}",
        g.fields
    );
}
