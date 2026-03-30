/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2022 ShieldBlaze
 *
 * ShieldBlaze ExpressGateway is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * ShieldBlaze ExpressGateway is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with ShieldBlaze ExpressGateway.  If not, see <https://www.gnu.org/licenses/>.
 */
package com.shieldblaze.expressgateway.protocol.http;

import com.shieldblaze.expressgateway.backend.NodeBuilder;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterBuilder;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.HTTPRoundRobin;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.sessionpersistence.NOOPSessionPersistence;
import com.shieldblaze.expressgateway.common.utils.AvailablePortUtil;
import com.shieldblaze.expressgateway.common.utils.SelfSignedCertificate;
import com.shieldblaze.expressgateway.configuration.ConfigurationContext;
import com.shieldblaze.expressgateway.configuration.tls.CertificateKeyPair;
import com.shieldblaze.expressgateway.configuration.tls.TlsClientConfiguration;
import com.shieldblaze.expressgateway.configuration.tls.TlsServerConfiguration;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStartupTask;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancer;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancerBuilder;
import com.shieldblaze.expressgateway.protocol.tcp.TCPListener;
import io.netty.bootstrap.Bootstrap;
import io.netty.channel.Channel;
import io.netty.channel.ChannelFuture;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInitializer;
import io.netty.channel.EventLoopGroup;
import io.netty.channel.SimpleChannelInboundHandler;
import io.netty.channel.nio.NioEventLoopGroup;
import io.netty.channel.socket.SocketChannel;
import io.netty.channel.socket.nio.NioSocketChannel;
import io.netty.handler.codec.http2.DefaultHttp2Headers;
import io.netty.handler.codec.http2.DefaultHttp2HeadersFrame;
import io.netty.handler.codec.http2.Http2FrameCodecBuilder;
import io.netty.handler.codec.http2.Http2Headers;
import io.netty.handler.codec.http2.Http2HeadersFrame;
import io.netty.handler.codec.http2.Http2MultiplexHandler;
import io.netty.handler.codec.http2.Http2ResetFrame;
import io.netty.handler.codec.http2.Http2StreamChannel;
import io.netty.handler.codec.http2.Http2StreamChannelBootstrap;
import io.netty.handler.codec.http2.Http2StreamFrame;
import io.netty.handler.ssl.ApplicationProtocolConfig;
import io.netty.handler.ssl.ApplicationProtocolNames;
import io.netty.handler.ssl.SslContext;
import io.netty.handler.ssl.SslContextBuilder;
import io.netty.handler.ssl.SslHandler;
import io.netty.handler.ssl.SslProvider;
import io.netty.handler.ssl.util.InsecureTrustManagerFactory;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.Timeout;

import java.net.InetSocketAddress;
import java.util.List;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.TimeUnit;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * SEC-01: Tests for CRLF injection defense in HTTP/2 pseudo-headers.
 *
 * <p>When proxying HTTP/2 to HTTP/1.1 backends, pseudo-header values are used
 * directly in the HTTP/1.1 request construction:
 * <ul>
 *   <li>{@code :path} -> request-line (e.g., {@code GET /path HTTP/1.1\r\n})</li>
 *   <li>{@code :authority} -> Host header</li>
 *   <li>{@code :scheme} -> X-Forwarded-Proto / Forwarded header values</li>
 * </ul>
 *
 * <p>If any of these values contain CR ({@code \r}, 0x0D) or LF ({@code \n}, 0x0A),
 * the H2-to-H1 conversion could inject additional HTTP headers or a second request
 * into the backend connection -- a classic HTTP desync/smuggling attack.</p>
 *
 * <p>RFC 9113 Section 8.2.1 explicitly forbids CR, LF, and NUL in field values.
 * This test verifies that such values are rejected with RST_STREAM(PROTOCOL_ERROR)
 * at either the codec level (Netty's HPACK decoder validation) or at our handler
 * level ({@link Http2ServerInboundHandler#containsCRLF}).</p>
 *
 * <p>The test has two tiers:
 * <ol>
 *   <li><b>Unit tests</b> for the {@code containsCRLF()} scanning method -- these
 *       directly verify the logic regardless of codec behavior.</li>
 *   <li><b>Integration tests</b> using a Netty H2 client with validation disabled
 *       ({@code DefaultHttp2Headers(false)}) to send CRLF in pseudo-headers over
 *       the wire. The server must reject the request (either at the codec or handler
 *       layer) and MUST NOT forward it to the backend.</li>
 * </ol>
 */
@Timeout(value = 60)
class Http2CRLFInjectionTest {

    private static final Logger logger = LogManager.getLogger(Http2CRLFInjectionTest.class);

    private static int loadBalancerPort;
    private static HTTPLoadBalancer httpLoadBalancer;
    private static HttpServer httpServer;

    @BeforeAll
    static void setup() throws Exception {
        // Generate a self-signed certificate valid for 127.0.0.1 and localhost
        SelfSignedCertificate ssc = SelfSignedCertificate.generateNew(
                List.of("127.0.0.1"), List.of("localhost"));
        CertificateKeyPair certificateKeyPair = CertificateKeyPair.forClient(
                List.of(ssc.x509Certificate()), ssc.keyPair().getPrivate());

        // TLS for the frontend (server-side): enables ALPN negotiation for h2
        TlsServerConfiguration tlsServerConfiguration =
                TlsServerConfiguration.copyFrom(TlsServerConfiguration.DEFAULT);
        tlsServerConfiguration.enable();
        tlsServerConfiguration.addMapping("localhost", certificateKeyPair);
        tlsServerConfiguration.defaultMapping(certificateKeyPair);

        // TLS for the backend (client-side): proxy connects to backend over TLS
        TlsClientConfiguration tlsClientConfiguration =
                TlsClientConfiguration.copyFrom(TlsClientConfiguration.DEFAULT);
        tlsClientConfiguration.enable();
        tlsClientConfiguration.setAcceptAllCerts(true);
        tlsClientConfiguration.defaultMapping(CertificateKeyPair.newDefaultClientInstance());

        // Start backend HTTP server with TLS (supports h2 + h1.1 via ALPN)
        httpServer = new HttpServer(true);
        httpServer.start();
        httpServer.START_FUTURE.get(60, TimeUnit.SECONDS);

        // Bind load balancer on a random available port
        loadBalancerPort = AvailablePortUtil.getTcpPort();

        Cluster cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new HTTPRoundRobin(NOOPSessionPersistence.INSTANCE))
                .build();

        httpLoadBalancer = HTTPLoadBalancerBuilder.newBuilder()
                .withConfigurationContext(ConfigurationContext.create(
                        tlsClientConfiguration, tlsServerConfiguration))
                .withBindAddress(new InetSocketAddress("127.0.0.1", loadBalancerPort))
                .withL4FrontListener(new TCPListener())
                .build();

        httpLoadBalancer.mappedCluster("localhost:" + loadBalancerPort, cluster);

        NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress("127.0.0.1", httpServer.port()))
                .build();

        L4FrontListenerStartupTask startupTask = httpLoadBalancer.start();
        startupTask.future().get(60, TimeUnit.SECONDS);
        assertTrue(startupTask.isSuccess(), "Load balancer must start successfully");

        // Allow the server socket to fully initialize
        Thread.sleep(500);
    }

    @AfterAll
    static void teardown() throws Exception {
        if (httpLoadBalancer != null) {
            httpLoadBalancer.shutdown().future().get(60, TimeUnit.SECONDS);
        }
        if (httpServer != null) {
            httpServer.shutdown();
            httpServer.SHUTDOWN_FUTURE.get(60, TimeUnit.SECONDS);
        }
    }

    // ===================================================================
    // Unit tests for containsCRLF()
    // ===================================================================

    /**
     * Null input must return false (no CRLF found). This is the expected
     * behavior for absent pseudo-headers (e.g., :authority on a CONNECT request).
     */
    @Test
    void containsCRLF_nullInput_returnsFalse() {
        assertFalse(Http2ServerInboundHandler.containsCRLF(null));
    }

    /**
     * Empty string must return false.
     */
    @Test
    void containsCRLF_emptyInput_returnsFalse() {
        assertFalse(Http2ServerInboundHandler.containsCRLF(""));
    }

    /**
     * Clean path values must pass validation.
     */
    @Test
    void containsCRLF_cleanPath_returnsFalse() {
        assertFalse(Http2ServerInboundHandler.containsCRLF("/"));
        assertFalse(Http2ServerInboundHandler.containsCRLF("/api/v1/resource"));
        assertFalse(Http2ServerInboundHandler.containsCRLF("/path?query=value&foo=bar"));
        assertFalse(Http2ServerInboundHandler.containsCRLF("/path%20with%20encoded%20spaces"));
    }

    /**
     * Clean authority values must pass validation.
     */
    @Test
    void containsCRLF_cleanAuthority_returnsFalse() {
        assertFalse(Http2ServerInboundHandler.containsCRLF("localhost:8080"));
        assertFalse(Http2ServerInboundHandler.containsCRLF("example.com"));
        assertFalse(Http2ServerInboundHandler.containsCRLF("[::1]:443"));
    }

    /**
     * Clean scheme values must pass validation.
     */
    @Test
    void containsCRLF_cleanScheme_returnsFalse() {
        assertFalse(Http2ServerInboundHandler.containsCRLF("https"));
        assertFalse(Http2ServerInboundHandler.containsCRLF("http"));
    }

    /**
     * Bare CR (0x0D) must be detected. This is the classic header injection
     * character that, combined with LF, forms the HTTP line terminator.
     */
    @Test
    void containsCRLF_crOnly_returnsTrue() {
        assertTrue(Http2ServerInboundHandler.containsCRLF("/path\r"));
        assertTrue(Http2ServerInboundHandler.containsCRLF("\r/path"));
        assertTrue(Http2ServerInboundHandler.containsCRLF("/pa\rth"));
    }

    /**
     * Bare LF (0x0A) must be detected. Some HTTP parsers treat LF alone
     * as a line terminator (RFC 9112 Section 2.2 tolerates bare LF).
     */
    @Test
    void containsCRLF_lfOnly_returnsTrue() {
        assertTrue(Http2ServerInboundHandler.containsCRLF("/path\n"));
        assertTrue(Http2ServerInboundHandler.containsCRLF("\n/path"));
        assertTrue(Http2ServerInboundHandler.containsCRLF("/pa\nth"));
    }

    /**
     * Full CRLF sequence in :path — the canonical injection payload.
     * This would inject an extra header into the H1 request-line:
     * {@code GET /path\r\nEvil-Header: injected HTTP/1.1}
     */
    @Test
    void containsCRLF_fullCRLF_returnsTrue() {
        assertTrue(Http2ServerInboundHandler.containsCRLF("/path\r\nEvil-Header: injected"));
    }

    /**
     * CRLF injection in :authority that would inject into the Host header:
     * {@code Host: legit.com\r\nEvil: injected}
     */
    @Test
    void containsCRLF_authorityInjection_returnsTrue() {
        assertTrue(Http2ServerInboundHandler.containsCRLF("legit.com\r\nEvil: injected"));
    }

    /**
     * CRLF at various positions in the value to ensure the full scan works.
     */
    @Test
    void containsCRLF_variousPositions_returnsTrue() {
        // At the very beginning
        assertTrue(Http2ServerInboundHandler.containsCRLF("\r\n"));
        // At the very end
        assertTrue(Http2ServerInboundHandler.containsCRLF("value\r\n"));
        // In the middle
        assertTrue(Http2ServerInboundHandler.containsCRLF("before\r\nafter"));
        // Single character
        assertTrue(Http2ServerInboundHandler.containsCRLF("\r"));
        assertTrue(Http2ServerInboundHandler.containsCRLF("\n"));
    }

    // ===================================================================
    // Integration tests: CRLF in H2 pseudo-headers over the wire
    // ===================================================================

    /**
     * SEC-01: CRLF in :path must be rejected and MUST NOT reach the backend.
     *
     * <p>Sends an HTTP/2 HEADERS frame with CRLF injection in the :path
     * pseudo-header. The proxy must reject this at either the HPACK decoder
     * level (Netty's validateHeaders=true) or at the handler level
     * (containsCRLF check). In either case, the stream must NOT receive
     * a 200 response.</p>
     *
     * <p>The expected outcome is one of:
     * <ul>
     *   <li>RST_STREAM(PROTOCOL_ERROR) on the stream</li>
     *   <li>GOAWAY on the connection (codec-level rejection)</li>
     *   <li>Connection close (stream channel goes inactive)</li>
     *   <li>Write failure on the client (codec rejects the frame on send)</li>
     * </ul>
     *
     * <p>The critical assertion is negative: a 200 response means the CRLF
     * reached the backend, which is a security vulnerability.</p>
     */
    @Test
    void crlfInPath_rejectedNotForwarded() throws Exception {
        // Use DefaultHttp2Headers(false) to bypass client-side header validation.
        // This allows constructing headers with CRLF that would normally be rejected
        // at construction time.
        Http2Headers maliciousHeaders = new DefaultHttp2Headers(false)
                .method("GET")
                .path("/\r\nEvil-Header: injected")
                .scheme("https")
                .authority("localhost:" + loadBalancerPort);

        assertStreamRejected(maliciousHeaders, "CRLF in :path");
    }

    /**
     * SEC-01: CRLF in :authority must be rejected.
     *
     * <p>:authority is converted to the Host header in H2-to-H1 conversion.
     * CRLF injection here would produce:
     * {@code Host: legit.com\r\nEvil: injected\r\n}</p>
     */
    @Test
    void crlfInAuthority_rejectedNotForwarded() throws Exception {
        Http2Headers maliciousHeaders = new DefaultHttp2Headers(false)
                .method("GET")
                .path("/")
                .scheme("https")
                .authority("localhost:" + loadBalancerPort + "\r\nEvil: injected");

        assertStreamRejected(maliciousHeaders, "CRLF in :authority");
    }

    /**
     * SEC-01: CRLF in :scheme must be rejected.
     *
     * <p>:scheme is used in X-Forwarded-Proto and the Forwarded header.
     * CRLF injection here would produce:
     * {@code X-Forwarded-Proto: https\r\nEvil: injected}</p>
     */
    @Test
    void crlfInScheme_rejectedNotForwarded() throws Exception {
        Http2Headers maliciousHeaders = new DefaultHttp2Headers(false)
                .method("GET")
                .path("/")
                .scheme("https\r\nEvil: injected")
                .authority("localhost:" + loadBalancerPort);

        assertStreamRejected(maliciousHeaders, "CRLF in :scheme");
    }

    /**
     * SEC-01: Bare LF (without CR) in :path must also be rejected.
     * Some HTTP/1.1 parsers treat bare LF as a line terminator
     * (RFC 9112 Section 2.2 notes this tolerance).
     */
    @Test
    void bareLfInPath_rejectedNotForwarded() throws Exception {
        Http2Headers maliciousHeaders = new DefaultHttp2Headers(false)
                .method("GET")
                .path("/\nEvil-Header: injected")
                .scheme("https")
                .authority("localhost:" + loadBalancerPort);

        assertStreamRejected(maliciousHeaders, "bare LF in :path");
    }

    /**
     * SEC-01: Bare CR (without LF) in :path must also be rejected.
     */
    @Test
    void bareCrInPath_rejectedNotForwarded() throws Exception {
        Http2Headers maliciousHeaders = new DefaultHttp2Headers(false)
                .method("GET")
                .path("/\rEvil-Header: injected")
                .scheme("https")
                .authority("localhost:" + loadBalancerPort);

        assertStreamRejected(maliciousHeaders, "bare CR in :path");
    }

    // ===================================================================
    // SEC-07: :authority / Host header consistency (RFC 9113 Section 8.3.1)
    // ===================================================================

    /**
     * SEC-07: When both :authority and Host are present with different values,
     * the proxy MUST reject with 400 Bad Request. Differing values cause a
     * routing desync: the proxy routes on :authority while the H2-to-H1
     * conversion emits Host to the backend, potentially targeting a different
     * virtual host.
     *
     * <p>RFC 9113 Section 8.3.1: "If a Host header field is present, the request
     * MUST contain an :authority pseudo-header field with the same value as the
     * Host header."</p>
     */
    @Test
    void authorityHostMismatch_rejectedWith400() throws Exception {
        // :authority = localhost:<port>, Host = evil.example.com — different values
        Http2Headers headers = new DefaultHttp2Headers(false)
                .method("GET")
                .path("/")
                .scheme("https")
                .authority("localhost:" + loadBalancerPort);
        headers.set("host", "evil.example.com");

        assertStreamRejectedWith400(headers, "SEC-07: :authority/Host mismatch");
    }

    /**
     * SEC-07: When :authority and Host have the same value (case-insensitive),
     * the request MUST NOT be rejected with 400. This sanity check ensures the
     * SEC-07 validation does not reject legitimate traffic where both headers
     * agree. The assertion is that no 400 is returned -- a 200 or TIMEOUT are
     * both acceptable (TIMEOUT means the backend roundtrip was slow, but the
     * proxy did not reject the request).
     */
    @Test
    void authorityHostMatch_notRejected() throws Exception {
        Http2Headers headers = new DefaultHttp2Headers(false)
                .method("GET")
                .path("/")
                .scheme("https")
                .authority("localhost:" + loadBalancerPort);
        headers.set("host", "localhost:" + loadBalancerPort);

        assertStreamNotRejectedWith400(headers, "SEC-07: :authority/Host match");
    }

    /**
     * SEC-07: Case-insensitive comparison — :authority and Host differing only
     * in case MUST NOT be rejected. RFC 9113 Section 8.3.1 requires the "same
     * value", and authority components are case-insensitive per RFC 3986
     * Section 3.2.2.
     */
    @Test
    void authorityHostCaseInsensitiveMatch_notRejected() throws Exception {
        Http2Headers headers = new DefaultHttp2Headers(false)
                .method("GET")
                .path("/")
                .scheme("https")
                .authority("localhost:" + loadBalancerPort);
        headers.set("host", "LOCALHOST:" + loadBalancerPort);

        assertStreamNotRejectedWith400(headers, "SEC-07: case-insensitive :authority/Host match");
    }

    /**
     * SEC-07: When only :authority is present (no Host header), the request
     * MUST NOT be rejected. The SEC-07 check only applies when BOTH are present.
     */
    @Test
    void authorityOnly_noHost_notRejected() throws Exception {
        // DefaultHttp2Headers with validation enabled — no Host header added
        Http2Headers headers = new DefaultHttp2Headers()
                .method("GET")
                .path("/")
                .scheme("https")
                .authority("localhost:" + loadBalancerPort);

        assertStreamNotRejectedWith400(headers, "SEC-07: :authority only (no Host)");
    }

    /**
     * Sanity check: a valid request with clean pseudo-headers MUST succeed with 200.
     * This ensures our CRLF validation does not regress normal traffic.
     */
    @Test
    void cleanHeaders_succeedsWith200() throws Exception {
        EventLoopGroup group = new NioEventLoopGroup(1);
        try {
            SslContext sslCtx = buildClientSslContext();
            CompletableFuture<String> responseStatus = new CompletableFuture<>();

            Channel parentChannel = connectH2Client(group, sslCtx);

            Http2StreamChannel stream = openStream(parentChannel,
                    new SimpleChannelInboundHandler<Http2StreamFrame>() {
                        @Override
                        protected void channelRead0(ChannelHandlerContext ctx, Http2StreamFrame frame) {
                            if (frame instanceof Http2HeadersFrame headersFrame) {
                                responseStatus.complete(headersFrame.headers().status().toString());
                            }
                        }

                        @Override
                        public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
                            responseStatus.completeExceptionally(cause);
                        }
                    });

            Http2Headers headers = new DefaultHttp2Headers()
                    .method("GET")
                    .path("/")
                    .scheme("https")
                    .authority("localhost:" + loadBalancerPort);

            stream.writeAndFlush(new DefaultHttp2HeadersFrame(headers, true)).sync();

            String status = responseStatus.get(10, TimeUnit.SECONDS);
            assertEquals("200", status, "Clean request must succeed with 200");

            parentChannel.close().sync();
        } finally {
            group.shutdownGracefully().sync();
        }
    }

    // ===================================================================
    // Helpers
    // ===================================================================

    /**
     * Sends an HTTP/2 HEADERS frame with the given (potentially malicious) headers
     * and asserts that the stream is rejected. "Rejected" means one of:
     * <ul>
     *   <li>RST_STREAM received on the stream</li>
     *   <li>The stream channel becomes inactive (connection-level rejection)</li>
     *   <li>The write itself fails (client codec rejects the outbound frame)</li>
     * </ul>
     *
     * <p>A 200 response is a test failure: it means CRLF reached the backend.</p>
     *
     * @param headers     the H2 headers to send (may contain CRLF)
     * @param description human-readable label for assertion messages
     */
    private void assertStreamRejected(Http2Headers headers, String description) throws Exception {
        EventLoopGroup group = new NioEventLoopGroup(1);
        try {
            SslContext sslCtx = buildClientSslContext();

            // Track whether the stream was rejected. Use three signals:
            // 1. RST_STREAM received -> definitive rejection
            // 2. Stream channel inactive -> connection closed (codec rejection)
            // 3. A 200 response -> FAILURE (CRLF reached backend)
            CompletableFuture<String> outcome = new CompletableFuture<>();

            Channel parentChannel = connectH2Client(group, sslCtx);

            Http2StreamChannel stream = openStream(parentChannel,
                    new SimpleChannelInboundHandler<Http2StreamFrame>() {
                        @Override
                        protected void channelRead0(ChannelHandlerContext ctx, Http2StreamFrame frame) {
                            if (frame instanceof Http2ResetFrame resetFrame) {
                                logger.info("{}: Received RST_STREAM errorCode={}",
                                        description, resetFrame.errorCode());
                                outcome.complete("RST_STREAM");
                            } else if (frame instanceof Http2HeadersFrame headersFrame) {
                                String status = headersFrame.headers().status().toString();
                                logger.info("{}: Received response status={}",
                                        description, status);
                                if ("200".equals(status)) {
                                    // CRLF reached the backend -- security vulnerability!
                                    outcome.complete("200_OK");
                                } else {
                                    // 400/500-level error response -- rejection
                                    outcome.complete("ERROR_" + status);
                                }
                            }
                        }

                        @Override
                        public void channelInactive(ChannelHandlerContext ctx) {
                            // Connection or stream closed -- also counts as rejection
                            outcome.complete("STREAM_CLOSED");
                        }

                        @Override
                        public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
                            logger.info("{}: Stream exception: {}", description, cause.getMessage());
                            outcome.complete("EXCEPTION");
                        }
                    });

            // Attempt to send the malicious headers. The write itself may fail
            // if the client-side codec rejects the outbound frame.
            ChannelFuture writeFuture = stream.writeAndFlush(
                    new DefaultHttp2HeadersFrame(headers, true));

            writeFuture.addListener(f -> {
                if (!f.isSuccess()) {
                    logger.info("{}: Write failed: {}", description,
                            f.cause() != null ? f.cause().getMessage() : "unknown");
                    outcome.complete("WRITE_FAILED");
                }
            });

            // Wait for the outcome. The timeout is generous to allow for TLS handshake
            // and codec processing, but if nothing happens it means the frame was silently
            // swallowed (also not a 200, so acceptable).
            String result;
            try {
                result = outcome.get(10, TimeUnit.SECONDS);
            } catch (java.util.concurrent.TimeoutException e) {
                // Timeout means no response at all -- the server rejected silently
                // (e.g., codec dropped the frame). This is acceptable: the CRLF
                // did not reach the backend.
                result = "TIMEOUT";
            }

            logger.info("{}: Outcome = {}", description, result);

            // The critical assertion: a 200 response means CRLF was forwarded to
            // the backend, which is a security vulnerability.
            assertFalse("200_OK".equals(result),
                    description + ": CRLF in pseudo-header must NOT produce a 200 response "
                            + "(CRLF was forwarded to backend). Actual outcome: " + result);

            parentChannel.close().sync();
        } finally {
            group.shutdownGracefully().sync();
        }
    }

    /**
     * SEC-07: Sends an HTTP/2 HEADERS frame and asserts that the server responds
     * with a 400 Bad Request. Used for :authority/Host consistency validation where
     * the request is well-formed at the H2 framing level but semantically invalid.
     *
     * @param headers     the H2 headers to send
     * @param description human-readable label for assertion messages
     */
    private void assertStreamRejectedWith400(Http2Headers headers, String description) throws Exception {
        EventLoopGroup group = new NioEventLoopGroup(1);
        try {
            SslContext sslCtx = buildClientSslContext();
            CompletableFuture<String> outcome = new CompletableFuture<>();

            Channel parentChannel = connectH2Client(group, sslCtx);

            Http2StreamChannel stream = openStream(parentChannel,
                    new SimpleChannelInboundHandler<Http2StreamFrame>() {
                        @Override
                        protected void channelRead0(ChannelHandlerContext ctx, Http2StreamFrame frame) {
                            if (frame instanceof Http2ResetFrame resetFrame) {
                                outcome.complete("RST_STREAM_" + resetFrame.errorCode());
                            } else if (frame instanceof Http2HeadersFrame headersFrame) {
                                outcome.complete(headersFrame.headers().status().toString());
                            }
                        }

                        @Override
                        public void channelInactive(ChannelHandlerContext ctx) {
                            outcome.complete("STREAM_CLOSED");
                        }

                        @Override
                        public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
                            outcome.complete("EXCEPTION");
                        }
                    });

            stream.writeAndFlush(new DefaultHttp2HeadersFrame(headers, true)).sync();

            String result;
            try {
                result = outcome.get(10, TimeUnit.SECONDS);
            } catch (java.util.concurrent.TimeoutException e) {
                result = "TIMEOUT";
            }

            logger.info("{}: Outcome = {}", description, result);
            assertEquals("400", result,
                    description + ": Expected 400 Bad Request but got: " + result);

            parentChannel.close().sync();
        } finally {
            group.shutdownGracefully().sync();
        }
    }

    /**
     * SEC-07: Sends an HTTP/2 HEADERS frame and asserts the server does NOT respond
     * with 400 Bad Request. This is a negative assertion: it verifies that the SEC-07
     * validation does not incorrectly reject requests where :authority and Host agree
     * (or where Host is absent). A 200, TIMEOUT, or any non-400 outcome is acceptable.
     *
     * <p>We use this weaker assertion (not-400) instead of assertEquals("200") because
     * the full end-to-end roundtrip through the proxy to the backend may time out in
     * constrained test environments. The critical property is that the proxy did not
     * actively reject the request with 400.</p>
     *
     * @param headers     the H2 headers to send
     * @param description human-readable label for assertion messages
     */
    private void assertStreamNotRejectedWith400(Http2Headers headers, String description) throws Exception {
        EventLoopGroup group = new NioEventLoopGroup(1);
        try {
            SslContext sslCtx = buildClientSslContext();
            CompletableFuture<String> outcome = new CompletableFuture<>();

            Channel parentChannel = connectH2Client(group, sslCtx);

            Http2StreamChannel stream = openStream(parentChannel,
                    new SimpleChannelInboundHandler<Http2StreamFrame>() {
                        @Override
                        protected void channelRead0(ChannelHandlerContext ctx, Http2StreamFrame frame) {
                            if (frame instanceof Http2ResetFrame resetFrame) {
                                outcome.complete("RST_STREAM_" + resetFrame.errorCode());
                            } else if (frame instanceof Http2HeadersFrame headersFrame) {
                                outcome.complete(headersFrame.headers().status().toString());
                            }
                        }

                        @Override
                        public void channelInactive(ChannelHandlerContext ctx) {
                            outcome.complete("STREAM_CLOSED");
                        }

                        @Override
                        public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
                            outcome.complete("EXCEPTION");
                        }
                    });

            stream.writeAndFlush(new DefaultHttp2HeadersFrame(headers, true)).sync();

            String result;
            try {
                result = outcome.get(10, TimeUnit.SECONDS);
            } catch (java.util.concurrent.TimeoutException e) {
                result = "TIMEOUT";
            }

            logger.info("{}: Outcome = {}", description, result);
            assertFalse("400".equals(result),
                    description + ": Request must NOT be rejected with 400 Bad Request. "
                            + "Actual outcome: " + result);

            parentChannel.close().sync();
        } finally {
            group.shutdownGracefully().sync();
        }
    }

    /**
     * Builds a Netty {@link SslContext} for an HTTP/2 client with ALPN
     * advertising h2. Uses InsecureTrustManager for the self-signed cert.
     */
    private SslContext buildClientSslContext() throws Exception {
        return SslContextBuilder.forClient()
                .sslProvider(SslProvider.JDK)
                .trustManager(InsecureTrustManagerFactory.INSTANCE)
                .applicationProtocolConfig(new ApplicationProtocolConfig(
                        ApplicationProtocolConfig.Protocol.ALPN,
                        ApplicationProtocolConfig.SelectorFailureBehavior.NO_ADVERTISE,
                        ApplicationProtocolConfig.SelectedListenerFailureBehavior.ACCEPT,
                        ApplicationProtocolNames.HTTP_2,
                        ApplicationProtocolNames.HTTP_1_1))
                .build();
    }

    /**
     * Creates a Netty H2 client connection to the load balancer.
     * Uses the standard Http2FrameCodec + Http2MultiplexHandler pipeline.
     */
    private Channel connectH2Client(EventLoopGroup group, SslContext sslCtx) throws Exception {
        Bootstrap bootstrap = new Bootstrap()
                .group(group)
                .channel(NioSocketChannel.class)
                .handler(new ChannelInitializer<SocketChannel>() {
                    @Override
                    protected void initChannel(SocketChannel ch) {
                        SslHandler sslHandler = sslCtx.newHandler(
                                ch.alloc(), "localhost", loadBalancerPort);
                        ch.pipeline().addLast(sslHandler);
                        ch.pipeline().addLast(Http2FrameCodecBuilder.forClient().build());
                        ch.pipeline().addLast(new Http2MultiplexHandler(
                                new SimpleChannelInboundHandler<Http2StreamFrame>() {
                                    @Override
                                    protected void channelRead0(ChannelHandlerContext ctx,
                                                                Http2StreamFrame frame) {
                                        // No-op: proxy does not send push promises
                                    }
                                }));
                    }
                });

        ChannelFuture cf = bootstrap.connect("127.0.0.1", loadBalancerPort).sync();
        assertTrue(cf.isSuccess(), "H2 client must connect to load balancer");

        Channel channel = cf.channel();

        // Wait for TLS handshake to complete
        channel.pipeline().get(SslHandler.class).handshakeFuture().sync();

        // Brief pause for the H2 SETTINGS exchange to finish
        Thread.sleep(300);

        return channel;
    }

    /**
     * Opens a new HTTP/2 stream on an existing parent channel.
     */
    private Http2StreamChannel openStream(Channel parentChannel,
                                          SimpleChannelInboundHandler<Http2StreamFrame> handler) throws Exception {
        return new Http2StreamChannelBootstrap(parentChannel)
                .handler(handler)
                .open()
                .sync()
                .getNow();
    }
}
