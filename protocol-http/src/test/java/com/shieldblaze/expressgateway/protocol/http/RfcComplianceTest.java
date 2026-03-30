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
import io.netty.handler.codec.http.DefaultHttpHeaders;
import io.netty.handler.codec.http.DefaultHttpHeadersFactory;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpHeaders;
import io.netty.handler.codec.http2.DefaultHttp2Headers;
import io.netty.handler.codec.http2.DefaultHttp2HeadersFrame;
import io.netty.handler.codec.http2.Http2Error;
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
import org.junit.jupiter.api.MethodOrderer;
import org.junit.jupiter.api.Nested;
import org.junit.jupiter.api.Order;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.TestMethodOrder;
import org.junit.jupiter.api.Timeout;

import java.io.BufferedReader;
import java.io.InputStreamReader;
import java.io.OutputStream;
import java.lang.reflect.Method;
import java.net.InetSocketAddress;
import java.net.Socket;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.TimeUnit;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * RFC compliance tests for specific behavior changes in the ExpressGateway
 * HTTP/1.1, HTTP/2, and HTTP/3 handlers.
 *
 * <p>This test class validates the following fixes:
 * <ul>
 *   <li><b>H1-06</b>: RFC 9110 Section 6.6.1 -- Date header on proxy-generated responses</li>
 *   <li><b>H1-05</b>: RFC 9110 Section 7.6.2 -- Negative Max-Forwards rejection</li>
 *   <li><b>H1-07</b>: RFC 7230 Section 6.1 -- Connection header multi-token parsing</li>
 *   <li><b>H3 NUL</b>: RFC 9114 Section 4.2 -- NUL byte validation in pseudo-headers</li>
 *   <li><b>H2 TE</b>: RFC 9113 Section 8.2.2 -- TE header validation (only "trailers" allowed)</li>
 *   <li><b>H2 CONNECT</b>: RFC 9113 Section 8.5 -- CONNECT with :scheme rejection</li>
 * </ul>
 *
 * <p>Tests are organized into nested classes by protocol layer. HTTP/1.1 tests use
 * raw TCP sockets (same pattern as {@link RequestSmugglingTest}). HTTP/2 tests use
 * Netty's Http2FrameCodec for frame-level control (same pattern as {@link Http2LifecycleTest}).
 * HTTP/3 and HopByHopHeaders tests use reflection/unit testing since they validate
 * private static utility methods or public utility classes.</p>
 */
@Timeout(value = 60)
class RfcComplianceTest {

    private static final Logger logger = LogManager.getLogger(RfcComplianceTest.class);

    // =========================================================================
    // HTTP/1.1 integration tests (raw socket, plaintext)
    // =========================================================================

    /**
     * HTTP/1.1 RFC compliance tests using the full load balancer stack.
     * Uses raw TCP sockets to craft requests that Java's HttpClient would reject.
     */
    @Nested
    @TestMethodOrder(MethodOrderer.OrderAnnotation.class)
    class Http1ComplianceTest {

        private static int loadBalancerPort;
        private static HTTPLoadBalancer httpLoadBalancer;
        private static HttpServer httpServer;

        @BeforeAll
        static void setup() throws Exception {
            httpServer = new HttpServer(false);
            httpServer.start();
            httpServer.START_FUTURE.get();

            loadBalancerPort = AvailablePortUtil.getTcpPort();

            Cluster cluster = ClusterBuilder.newBuilder()
                    .withLoadBalance(new HTTPRoundRobin(NOOPSessionPersistence.INSTANCE))
                    .build();

            httpLoadBalancer = HTTPLoadBalancerBuilder.newBuilder()
                    .withConfigurationContext(ConfigurationContext.DEFAULT)
                    .withBindAddress(new InetSocketAddress("127.0.0.1", loadBalancerPort))
                    .withL4FrontListener(new TCPListener())
                    .build();

            httpLoadBalancer.mappedCluster("localhost:" + loadBalancerPort, cluster);

            NodeBuilder.newBuilder()
                    .withCluster(cluster)
                    .withSocketAddress(new InetSocketAddress("127.0.0.1", httpServer.port()))
                    .build();

            L4FrontListenerStartupTask startupTask = httpLoadBalancer.start();
            startupTask.future().join();
            assertTrue(startupTask.isSuccess());

            Thread.sleep(500);
        }

        @AfterAll
        static void shutdown() throws Exception {
            httpLoadBalancer.shutdown().future().get();
            httpServer.shutdown();
            httpServer.SHUTDOWN_FUTURE.get();
        }

        // =================================================================
        // H1-06: Date header on proxy-generated responses
        // RFC 9110 Section 6.6.1
        // =================================================================

        /**
         * H1-06: A 400 Bad Request response generated by the proxy (e.g., for a
         * missing Host header) MUST include a Date header per RFC 9110 Section 6.6.1.
         *
         * <p>Before this fix, proxy-generated error responses omitted the Date header.
         * This violates the RFC which states: "An origin server MUST send a Date header
         * field in all 2xx and 3xx responses, and SHOULD send it in all other responses
         * (with the exception of 1xx)."</p>
         */
        @Test
        @Order(1)
        void h1_06_400Response_includesDateHeader() throws Exception {
            // Missing Host header triggers 400 from the proxy
            String rawRequest = "GET / HTTP/1.1\r\n" +
                    "\r\n";

            List<String> responseLines = sendRawRequestAndReadAllHeaders(rawRequest);
            assertFalse(responseLines.isEmpty(), "Response must not be empty");
            assertTrue(responseLines.get(0).contains("400"),
                    "Missing Host must produce 400 but got: " + responseLines.get(0));

            boolean hasDate = responseLines.stream()
                    .anyMatch(line -> line.toLowerCase().startsWith("date:"));
            assertTrue(hasDate,
                    "H1-06: 400 response must include Date header. Response headers: " + responseLines);
        }

        /**
         * H1-06: A 405 Method Not Allowed response (TRACE blocked) must include Date header.
         * This tests a different error code path to ensure all proxy-generated responses
         * got the Date header fix.
         */
        @Test
        @Order(2)
        void h1_06_405Response_includesDateHeader() throws Exception {
            // TRACE is blocked by the proxy with 405 Method Not Allowed
            String rawRequest = "TRACE / HTTP/1.1\r\n" +
                    "Host: localhost:" + loadBalancerPort + "\r\n" +
                    "\r\n";

            List<String> responseLines = sendRawRequestAndReadAllHeaders(rawRequest);
            assertFalse(responseLines.isEmpty(), "Response must not be empty");

            String statusLine = responseLines.get(0);
            assertTrue(statusLine.contains("405"),
                    "TRACE must produce 405 but got: " + statusLine);

            boolean hasDate = responseLines.stream()
                    .anyMatch(line -> line.toLowerCase().startsWith("date:"));
            assertTrue(hasDate,
                    "H1-06: 405 response must include Date header. Response headers: " + responseLines);
        }

        /**
         * H1-06: A 400 response from duplicate Host header must include Date header.
         * Tests yet another error path (H1-02 validation).
         */
        @Test
        @Order(3)
        void h1_06_duplicateHostResponse_includesDateHeader() throws Exception {
            String rawRequest = "GET / HTTP/1.1\r\n" +
                    "Host: localhost:" + loadBalancerPort + "\r\n" +
                    "Host: localhost:" + loadBalancerPort + "\r\n" +
                    "\r\n";

            List<String> responseLines = sendRawRequestAndReadAllHeaders(rawRequest);
            assertFalse(responseLines.isEmpty(), "Response must not be empty");

            assertTrue(responseLines.get(0).contains("400"),
                    "Duplicate Host must produce 400 but got: " + responseLines.get(0));

            boolean hasDate = responseLines.stream()
                    .anyMatch(line -> line.toLowerCase().startsWith("date:"));
            assertTrue(hasDate,
                    "H1-06: 400 from duplicate Host must include Date header. Response headers: " + responseLines);
        }

        /**
         * H1-06: Verify the Date header format is RFC 1123 compliant.
         * RFC 9110 Section 5.6.7 requires the format: "Wed, 09 Jun 2021 10:18:14 GMT"
         */
        @Test
        @Order(4)
        void h1_06_dateHeader_hasRfc1123Format() throws Exception {
            String rawRequest = "GET / HTTP/1.1\r\n" +
                    "\r\n";

            List<String> responseLines = sendRawRequestAndReadAllHeaders(rawRequest);
            String dateLine = responseLines.stream()
                    .filter(line -> line.toLowerCase().startsWith("date:"))
                    .findFirst()
                    .orElse(null);

            assertNotNull(dateLine, "Date header must be present");

            String dateValue = dateLine.substring(dateLine.indexOf(':') + 1).trim();
            // RFC 1123 date format: "Thu, 25 Mar 2026 12:00:00 GMT"
            // Must contain day-of-week abbreviation, day number, month abbreviation, year, time, and GMT
            assertTrue(dateValue.contains("GMT"),
                    "Date must be in GMT timezone but got: " + dateValue);
            assertTrue(dateValue.matches("\\w{3}, \\d{1,2} \\w{3} \\d{4} \\d{2}:\\d{2}:\\d{2} GMT"),
                    "Date must match RFC 1123 format (e.g., 'Wed, 25 Mar 2026 12:00:00 GMT') but got: " + dateValue);
        }

        // =================================================================
        // H1-05: Negative Max-Forwards rejection
        // RFC 9110 Section 7.6.2
        // =================================================================

        /**
         * GAP-H1-05: OPTIONS with Max-Forwards: -1 must be rejected with 400.
         *
         * <p>RFC 9110 Section 7.6.2 defines Max-Forwards as 1*DIGIT, which is
         * inherently non-negative. A negative value is malformed and must be
         * rejected to prevent undefined forwarding behavior.</p>
         */
        @Test
        @Order(5)
        void h1_05_negativeMaxForwards_returns400() throws Exception {
            String rawRequest = "OPTIONS / HTTP/1.1\r\n" +
                    "Host: localhost:" + loadBalancerPort + "\r\n" +
                    "Max-Forwards: -1\r\n" +
                    "\r\n";

            List<String> responseLines = sendRawRequestAndReadAllHeaders(rawRequest);
            assertFalse(responseLines.isEmpty(), "Response must not be empty");

            assertTrue(responseLines.get(0).contains("400"),
                    "H1-05: Negative Max-Forwards must produce 400 but got: " + responseLines.get(0));

            // Also verify Date header is present on this 400 (regression for H1-06)
            boolean hasDate = responseLines.stream()
                    .anyMatch(line -> line.toLowerCase().startsWith("date:"));
            assertTrue(hasDate,
                    "H1-06 regression: 400 from Max-Forwards check must include Date header");
        }

        /**
         * Positive regression test: Max-Forwards: 0 on OPTIONS must be handled
         * locally (not forwarded to backend) with a 200 response.
         */
        @Test
        @Order(6)
        void h1_05_zeroMaxForwards_handledLocally() throws Exception {
            String rawRequest = "OPTIONS / HTTP/1.1\r\n" +
                    "Host: localhost:" + loadBalancerPort + "\r\n" +
                    "Max-Forwards: 0\r\n" +
                    "\r\n";

            List<String> responseLines = sendRawRequestAndReadAllHeaders(rawRequest);
            assertFalse(responseLines.isEmpty(), "Response must not be empty");

            assertTrue(responseLines.get(0).contains("200"),
                    "Max-Forwards: 0 on OPTIONS must be handled locally with 200 but got: " + responseLines.get(0));

            // Must have Allow header listing supported methods
            boolean hasAllow = responseLines.stream()
                    .anyMatch(line -> line.toLowerCase().startsWith("allow:"));
            assertTrue(hasAllow,
                    "Local OPTIONS response must include Allow header");
        }

        /**
         * Positive regression test: Max-Forwards: 5 on OPTIONS must be forwarded
         * to the backend after decrementing.
         */
        @Test
        @Order(7)
        void h1_05_positiveMaxForwards_forwarded() throws Exception {
            String rawRequest = "OPTIONS / HTTP/1.1\r\n" +
                    "Host: localhost:" + loadBalancerPort + "\r\n" +
                    "Max-Forwards: 5\r\n" +
                    "\r\n";

            List<String> responseLines = sendRawRequestAndReadAllHeaders(rawRequest);
            assertFalse(responseLines.isEmpty(), "Response must not be empty");

            // Should be forwarded to backend and get a 200
            assertTrue(responseLines.get(0).contains("200"),
                    "Max-Forwards: 5 on OPTIONS should be forwarded and succeed but got: " + responseLines.get(0));
        }

        /**
         * Sends a raw HTTP request and returns all response header lines
         * (status line + headers, up to the empty line that ends headers).
         */
        private List<String> sendRawRequestAndReadAllHeaders(String rawRequest) throws Exception {
            List<String> lines = new ArrayList<>();
            try (Socket socket = new Socket("127.0.0.1", loadBalancerPort)) {
                socket.setSoTimeout(5000);

                OutputStream out = socket.getOutputStream();
                out.write(rawRequest.getBytes(StandardCharsets.UTF_8));
                out.flush();

                try (BufferedReader reader = new BufferedReader(
                        new InputStreamReader(socket.getInputStream(), StandardCharsets.UTF_8))) {

                    String line;
                    while ((line = reader.readLine()) != null) {
                        if (line.isEmpty()) {
                            break; // End of headers
                        }
                        lines.add(line);
                    }
                }
            }
            return lines;
        }
    }

    // =========================================================================
    // H1-07: Connection header multi-token parsing (unit test)
    // =========================================================================

    /**
     * Unit tests for {@link HopByHopHeaders#strip(HttpHeaders)} verifying that
     * the Connection header's comma-separated token list is correctly parsed and
     * the named headers are removed.
     *
     * <p>RFC 7230 Section 6.1: "Each token in the Connection header field value
     * corresponds to a header field name that MUST be removed." The fix ensures
     * multi-token values like "close, X-Custom" are parsed correctly with the
     * zero-alloc comma scanning approach.</p>
     */
    @Nested
    class ConnectionTokenParsingTest {

        /**
         * Single token: "Connection: close" should cause removal of any header
         * named "close" (even though "close" is not typically a separate header).
         * The Connection header itself is always removed.
         */
        @Test
        void singleToken_connectionHeaderRemoved() {
            HttpHeaders headers = new DefaultHttpHeaders();
            headers.add(HttpHeaderNames.CONNECTION, "close");
            headers.add("close", "should-be-removed");
            headers.add("x-keep", "should-survive");

            HopByHopHeaders.strip(headers);

            assertFalse(headers.contains(HttpHeaderNames.CONNECTION),
                    "Connection header must be removed");
            assertFalse(headers.contains("close"),
                    "Header named in Connection value must be removed");
            assertTrue(headers.contains("x-keep"),
                    "Unrelated header must survive");
        }

        /**
         * Multi-token: "Connection: keep-alive, X-Custom-Hop" should remove
         * both the keep-alive and X-Custom-Hop headers.
         */
        @Test
        void multipleTokens_allNamedHeadersRemoved() {
            HttpHeaders headers = new DefaultHttpHeaders();
            headers.add(HttpHeaderNames.CONNECTION, "keep-alive, X-Custom-Hop");
            headers.add("keep-alive", "timeout=5");
            headers.add("x-custom-hop", "value");
            headers.add("x-end-to-end", "should-survive");

            HopByHopHeaders.strip(headers);

            assertFalse(headers.contains(HttpHeaderNames.CONNECTION),
                    "Connection header must be removed");
            assertFalse(headers.contains("keep-alive"),
                    "keep-alive (named in Connection) must be removed");
            assertFalse(headers.contains("x-custom-hop"),
                    "X-Custom-Hop (named in Connection) must be removed");
            assertTrue(headers.contains("x-end-to-end"),
                    "End-to-end header must survive");
        }

        /**
         * Tokens with extra whitespace around commas:
         * "Connection: keep-alive , X-Foo" must be trimmed correctly.
         *
         * <p>Uses {@code DefaultHttpHeaders(false)} to disable Netty's header value
         * validation, which would otherwise reject values with certain whitespace
         * patterns. In production, raw HTTP bytes may contain such whitespace
         * before Netty normalizes them.</p>
         */
        @Test
        void tokensWithWhitespace_trimmedAndRemoved() {
            // Disable Netty header validation to allow whitespace in Connection value.
            // This is intentional: the test verifies that HopByHopHeaders.strip() correctly
            // trims tokens even when raw HTTP bytes contain extra whitespace around commas,
            // which Netty's default validator would reject before we can test the stripping logic.
            HttpHeaders headers = DefaultHttpHeadersFactory.headersFactory()
                    .withValidation(false)
                    .newHeaders();
            headers.add(HttpHeaderNames.CONNECTION, "keep-alive , X-Foo");
            headers.add("keep-alive", "timeout=5");
            headers.add("x-foo", "bar");
            headers.add("x-keep", "survive");

            HopByHopHeaders.strip(headers);

            assertFalse(headers.contains("keep-alive"),
                    "keep-alive must be removed despite whitespace around comma");
            assertFalse(headers.contains("x-foo"),
                    "X-Foo must be removed despite whitespace around comma");
            assertTrue(headers.contains("x-keep"),
                    "Unrelated header must survive");
        }

        /**
         * Standard hop-by-hop headers (TE, Trailers, Proxy-Authenticate, etc.)
         * must be removed even when not listed in Connection.
         */
        @Test
        void standardHopByHopHeaders_alwaysRemoved() {
            HttpHeaders headers = new DefaultHttpHeaders();
            headers.add(HttpHeaderNames.TE, "trailers");
            headers.add("trailers", "");
            headers.add(HttpHeaderNames.PROXY_AUTHENTICATE, "Basic");
            headers.add(HttpHeaderNames.PROXY_AUTHORIZATION, "Bearer xxx");
            headers.add(HttpHeaderNames.UPGRADE, "h2c");
            headers.add(HttpHeaderNames.TRANSFER_ENCODING, "chunked");
            headers.add("x-end-to-end", "survive");

            HopByHopHeaders.strip(headers);

            assertFalse(headers.contains(HttpHeaderNames.TE),
                    "TE must be removed");
            assertFalse(headers.contains("trailers"),
                    "Trailers must be removed");
            assertFalse(headers.contains(HttpHeaderNames.PROXY_AUTHENTICATE),
                    "Proxy-Authenticate must be removed");
            assertFalse(headers.contains(HttpHeaderNames.PROXY_AUTHORIZATION),
                    "Proxy-Authorization must be removed");
            assertFalse(headers.contains(HttpHeaderNames.UPGRADE),
                    "Upgrade must be removed (non-WebSocket)");
            assertFalse(headers.contains(HttpHeaderNames.TRANSFER_ENCODING),
                    "Transfer-Encoding must be removed on request path");
            assertTrue(headers.contains("x-end-to-end"),
                    "End-to-end header must survive");
        }

        /**
         * Empty Connection header value must not cause errors.
         */
        @Test
        void emptyConnectionValue_noError() {
            HttpHeaders headers = new DefaultHttpHeaders();
            headers.add(HttpHeaderNames.CONNECTION, "");
            headers.add("x-keep", "survive");

            HopByHopHeaders.strip(headers);

            assertFalse(headers.contains(HttpHeaderNames.CONNECTION),
                    "Connection header must be removed");
            assertTrue(headers.contains("x-keep"),
                    "Unrelated header must survive");
        }

        /**
         * Upgrade token in Connection is preserved when this is a WebSocket upgrade.
         */
        @Test
        void webSocketUpgrade_upgradeHeaderPreserved() {
            HttpHeaders headers = new DefaultHttpHeaders();
            headers.add(HttpHeaderNames.CONNECTION, "Upgrade");
            headers.add(HttpHeaderNames.UPGRADE, "websocket");
            headers.add(HttpHeaderNames.SEC_WEBSOCKET_KEY, "dGhlIHNhbXBsZSBub25jZQ==");

            HopByHopHeaders.strip(headers);

            // Upgrade header should be preserved for WebSocket
            assertTrue(headers.contains(HttpHeaderNames.UPGRADE),
                    "Upgrade header must be preserved for WebSocket upgrades");
        }
    }

    // =========================================================================
    // H3 NUL byte validation (reflection test)
    // =========================================================================

    /**
     * Tests the {@code containsProhibitedChars} private static method in
     * {@link Http3ServerHandler} via reflection.
     *
     * <p>RFC 9114 Section 4.2: field values MUST NOT contain NUL (0x00),
     * CR (0x0d), or LF (0x0a). This method is the validation gate that prevents
     * CRLF injection when translating HTTP/3 pseudo-headers to HTTP/1.1 for
     * backend forwarding.</p>
     */
    @Nested
    class Http3NulByteValidationTest {

        private static Method containsProhibitedChars;

        @BeforeAll
        static void findMethod() throws Exception {
            // Http3ServerHandler is package-private in its package, so we use Class.forName
            // and setAccessible to reach the private static containsProhibitedChars method.
            Class<?> h3HandlerClass = Class.forName(
                    "com.shieldblaze.expressgateway.protocol.http.http3.Http3ServerHandler");
            containsProhibitedChars = h3HandlerClass.getDeclaredMethod(
                    "containsProhibitedChars", CharSequence.class);
            containsProhibitedChars.setAccessible(true);
        }

        private boolean invokeContainsProhibitedChars(CharSequence value) throws Exception {
            return (boolean) containsProhibitedChars.invoke(null, value);
        }

        /**
         * Clean path with no prohibited characters must return false.
         */
        @Test
        void cleanPath_noProblem() throws Exception {
            assertFalse(invokeContainsProhibitedChars("/api/v1/users"),
                    "Clean path must not be flagged");
        }

        /**
         * Null input must return false (null-safe).
         */
        @Test
        void nullInput_returnsFalse() throws Exception {
            assertFalse(invokeContainsProhibitedChars(null),
                    "null must return false");
        }

        /**
         * NUL byte (0x00) in path must be detected.
         */
        @Test
        void nulByte_detected() throws Exception {
            assertTrue(invokeContainsProhibitedChars("/path\0injected"),
                    "NUL byte must be detected");
        }

        /**
         * CR (0x0d) in path must be detected.
         */
        @Test
        void carriageReturn_detected() throws Exception {
            assertTrue(invokeContainsProhibitedChars("/path\rinjected"),
                    "CR must be detected");
        }

        /**
         * LF (0x0a) in path must be detected.
         */
        @Test
        void lineFeed_detected() throws Exception {
            assertTrue(invokeContainsProhibitedChars("/path\ninjected"),
                    "LF must be detected");
        }

        /**
         * CRLF sequence (classic header injection) must be detected.
         */
        @Test
        void crlfInjection_detected() throws Exception {
            assertTrue(invokeContainsProhibitedChars("/path\r\nX-Injected: evil"),
                    "CRLF header injection must be detected");
        }

        /**
         * Empty string must not be flagged.
         */
        @Test
        void emptyString_noProblem() throws Exception {
            assertFalse(invokeContainsProhibitedChars(""),
                    "Empty string must not be flagged");
        }

        /**
         * NUL byte at the very start of the value.
         */
        @Test
        void nulAtStart_detected() throws Exception {
            assertTrue(invokeContainsProhibitedChars("\0/path"),
                    "NUL at start must be detected");
        }

        /**
         * NUL byte at the very end of the value.
         */
        @Test
        void nulAtEnd_detected() throws Exception {
            assertTrue(invokeContainsProhibitedChars("/path\0"),
                    "NUL at end must be detected");
        }
    }

    // =========================================================================
    // HTTP/2 compliance tests (TLS + H2 frame-level client)
    // =========================================================================

    /**
     * HTTP/2 RFC compliance tests using Netty's Http2FrameCodec for frame-level
     * control. Requires TLS with ALPN for H2 negotiation.
     *
     * <p>These tests validate:
     * <ul>
     *   <li><b>GAP-H2-01</b>: TE header with non-"trailers" value causes RST_STREAM</li>
     *   <li><b>GAP-H2-02</b>: CONNECT with :scheme causes RST_STREAM</li>
     * </ul>
     */
    @Nested
    @TestMethodOrder(MethodOrderer.OrderAnnotation.class)
    class Http2ComplianceTest {

        private static int loadBalancerPort;
        private static HTTPLoadBalancer httpLoadBalancer;
        private static HttpServer httpServer;

        @BeforeAll
        static void setup() throws Exception {
            SelfSignedCertificate ssc = SelfSignedCertificate.generateNew(
                    List.of("127.0.0.1"), List.of("localhost"));
            CertificateKeyPair certificateKeyPair = CertificateKeyPair.forClient(
                    List.of(ssc.x509Certificate()), ssc.keyPair().getPrivate());

            TlsServerConfiguration tlsServerConfiguration =
                    TlsServerConfiguration.copyFrom(TlsServerConfiguration.DEFAULT);
            tlsServerConfiguration.enable();
            tlsServerConfiguration.addMapping("localhost", certificateKeyPair);
            tlsServerConfiguration.defaultMapping(certificateKeyPair);

            TlsClientConfiguration tlsClientConfiguration =
                    TlsClientConfiguration.copyFrom(TlsClientConfiguration.DEFAULT);
            tlsClientConfiguration.enable();
            tlsClientConfiguration.setAcceptAllCerts(true);
            tlsClientConfiguration.defaultMapping(CertificateKeyPair.newDefaultClientInstance());

            httpServer = new HttpServer(true);
            httpServer.start();
            httpServer.START_FUTURE.get(60, TimeUnit.SECONDS);

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

        // =================================================================
        // Sanity: Normal H2 GET succeeds
        // =================================================================

        /**
         * Baseline test: A well-formed H2 GET request must succeed with 200.
         * If this fails, the H2 stack is broken and other tests are meaningless.
         */
        @Test
        @Order(1)
        void h2_sanity_normalGet_returns200() throws Exception {
            EventLoopGroup group = new NioEventLoopGroup(1);
            try {
                SslContext sslCtx = buildClientSslContext();
                String authority = "localhost:" + loadBalancerPort;

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
                        });

                Http2Headers headers = new DefaultHttp2Headers()
                        .method("GET")
                        .path("/")
                        .scheme("https")
                        .authority(authority);
                stream.writeAndFlush(new DefaultHttp2HeadersFrame(headers, true)).sync();

                String status = responseStatus.get(10, TimeUnit.SECONDS);
                assertEquals("200", status, "Normal H2 GET must return 200");

                parentChannel.close().sync();
            } finally {
                group.shutdownGracefully().sync();
            }
        }

        // =================================================================
        // GAP-H2-01: TE header validation
        // RFC 9113 Section 8.2.2
        // =================================================================

        /**
         * GAP-H2-01: Sending TE: gzip in an HTTP/2 request must be rejected.
         *
         * <p>RFC 9113 Section 8.2.2: "The only exception to this is the TE header
         * field, which MAY be present in an HTTP/2 request; when it is, it MUST NOT
         * contain any value other than 'trailers'."</p>
         *
         * <p>TE: gzip is valid in HTTP/1.1 but forbidden in HTTP/2. Netty's
         * Http2FrameCodec may reject this at the codec level (before the handler
         * sees it), or the handler may reject it with RST_STREAM. In either case,
         * the client stream is reset. The RST_STREAM may arrive as a
         * {@link Http2ResetFrame}, an exception in {@code exceptionCaught}, or
         * a channel closure -- all indicate rejection.</p>
         */
        @Test
        @Order(2)
        void h2_te_gzip_rejected() throws Exception {
            EventLoopGroup group = new NioEventLoopGroup(1);
            try {
                SslContext sslCtx = buildClientSslContext();
                String authority = "localhost:" + loadBalancerPort;

                // "rejected" completes with any signal that the stream was rejected:
                // RST_STREAM frame, exception, or channel closure.
                CompletableFuture<String> rejected = new CompletableFuture<>();

                Channel parentChannel = connectH2Client(group, sslCtx);

                Http2StreamChannel stream = openStream(parentChannel,
                        new SimpleChannelInboundHandler<Http2StreamFrame>() {
                            @Override
                            protected void channelRead0(ChannelHandlerContext ctx, Http2StreamFrame frame) {
                                if (frame instanceof Http2ResetFrame rstFrame) {
                                    rejected.complete("RST_STREAM:" + rstFrame.errorCode());
                                } else if (frame instanceof Http2HeadersFrame headersFrame) {
                                    String status = headersFrame.headers().status().toString();
                                    if ("200".equals(status)) {
                                        rejected.complete("ACCEPTED:" + status); // Should NOT happen
                                    } else {
                                        rejected.complete("STATUS:" + status);
                                    }
                                }
                            }

                            @Override
                            public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
                                // RST_STREAM may arrive as a stream exception
                                rejected.complete("EXCEPTION:" + cause.getClass().getSimpleName());
                            }

                            @Override
                            public void channelInactive(ChannelHandlerContext ctx) {
                                // Stream closed without explicit frame delivery
                                rejected.complete("CLOSED");
                            }
                        });

                Http2Headers headers = new DefaultHttp2Headers()
                        .method("GET")
                        .path("/")
                        .scheme("https")
                        .authority(authority);
                headers.set("te", "gzip");

                stream.writeAndFlush(new DefaultHttp2HeadersFrame(headers, true)).sync();

                String result = rejected.get(10, TimeUnit.SECONDS);
                logger.info("H2 TE:gzip test result: {}", result);

                // The request MUST NOT succeed with 200
                assertFalse(result.startsWith("ACCEPTED"),
                        "TE: gzip must NOT be accepted in HTTP/2 but got: " + result);
                // Must be rejected via RST_STREAM, error status, exception, or stream closure
                assertTrue(result.startsWith("RST_STREAM") || result.startsWith("STATUS")
                                || result.startsWith("EXCEPTION") || result.equals("CLOSED"),
                        "TE: gzip must be rejected but got: " + result);

                parentChannel.close().sync();
            } finally {
                group.shutdownGracefully().sync();
            }
        }

        /**
         * Positive regression: TE: trailers in HTTP/2 must be accepted (it is the
         * only permitted value per RFC 9113 Section 8.2.2).
         */
        @Test
        @Order(3)
        void h2_te_trailers_accepted() throws Exception {
            EventLoopGroup group = new NioEventLoopGroup(1);
            try {
                SslContext sslCtx = buildClientSslContext();
                String authority = "localhost:" + loadBalancerPort;

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
                        });

                Http2Headers headers = new DefaultHttp2Headers()
                        .method("GET")
                        .path("/")
                        .scheme("https")
                        .authority(authority);
                // TE: trailers is the only permitted value in H2
                headers.set("te", "trailers");

                stream.writeAndFlush(new DefaultHttp2HeadersFrame(headers, true)).sync();

                String status = responseStatus.get(10, TimeUnit.SECONDS);
                assertEquals("200", status,
                        "TE: trailers must be accepted in HTTP/2 but got: " + status);

                parentChannel.close().sync();
            } finally {
                group.shutdownGracefully().sync();
            }
        }

        // =================================================================
        // GAP-H2-02: CONNECT with :scheme validation
        // RFC 9113 Section 8.5
        // =================================================================

        /**
         * GAP-H2-02: A plain CONNECT request that includes :scheme must be rejected.
         *
         * <p>RFC 9113 Section 8.5: "The ':scheme' and ':path' pseudo-header fields
         * MUST be omitted" for CONNECT requests. Their presence indicates a malformed
         * CONNECT request.</p>
         *
         * <p>Note: Extended CONNECT (RFC 8441) with :protocol IS allowed to include
         * :scheme and :path. This test only sends plain CONNECT (no :protocol).</p>
         *
         * <p>The server sends RST_STREAM(PROTOCOL_ERROR) which may arrive as a
         * frame, exception, or channel closure on the client stream.</p>
         */
        @Test
        @Order(4)
        void h2_connect_withScheme_rejected() throws Exception {
            EventLoopGroup group = new NioEventLoopGroup(1);
            try {
                SslContext sslCtx = buildClientSslContext();
                String authority = "localhost:" + loadBalancerPort;

                CompletableFuture<String> rejected = new CompletableFuture<>();

                Channel parentChannel = connectH2Client(group, sslCtx);

                Http2StreamChannel stream = openStream(parentChannel,
                        new SimpleChannelInboundHandler<Http2StreamFrame>() {
                            @Override
                            protected void channelRead0(ChannelHandlerContext ctx, Http2StreamFrame frame) {
                                if (frame instanceof Http2ResetFrame rstFrame) {
                                    rejected.complete("RST_STREAM:" + rstFrame.errorCode());
                                } else if (frame instanceof Http2HeadersFrame headersFrame) {
                                    String status = headersFrame.headers().status().toString();
                                    rejected.complete("STATUS:" + status);
                                }
                            }

                            @Override
                            public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
                                rejected.complete("EXCEPTION:" + cause.getClass().getSimpleName());
                            }

                            @Override
                            public void channelInactive(ChannelHandlerContext ctx) {
                                rejected.complete("CLOSED");
                            }
                        });

                // Plain CONNECT with :scheme (forbidden per RFC 9113 Section 8.5)
                Http2Headers headers = new DefaultHttp2Headers()
                        .method("CONNECT")
                        .authority(authority);
                headers.scheme("https");

                stream.writeAndFlush(new DefaultHttp2HeadersFrame(headers, true)).sync();

                String result = rejected.get(10, TimeUnit.SECONDS);
                logger.info("H2 CONNECT+:scheme test result: {}", result);

                // Must not succeed
                assertFalse(result.equals("STATUS:200"),
                        "CONNECT with :scheme must NOT succeed but got 200");
                // Must be rejected
                assertTrue(result.startsWith("RST_STREAM") || result.startsWith("STATUS")
                                || result.startsWith("EXCEPTION") || result.equals("CLOSED"),
                        "CONNECT with :scheme must be rejected but got: " + result);

                parentChannel.close().sync();
            } finally {
                group.shutdownGracefully().sync();
            }
        }

        /**
         * GAP-H2-02: CONNECT with :path (without :protocol) must also be rejected.
         */
        @Test
        @Order(5)
        void h2_connect_withPath_rejected() throws Exception {
            EventLoopGroup group = new NioEventLoopGroup(1);
            try {
                SslContext sslCtx = buildClientSslContext();
                String authority = "localhost:" + loadBalancerPort;

                CompletableFuture<String> rejected = new CompletableFuture<>();

                Channel parentChannel = connectH2Client(group, sslCtx);

                Http2StreamChannel stream = openStream(parentChannel,
                        new SimpleChannelInboundHandler<Http2StreamFrame>() {
                            @Override
                            protected void channelRead0(ChannelHandlerContext ctx, Http2StreamFrame frame) {
                                if (frame instanceof Http2ResetFrame rstFrame) {
                                    rejected.complete("RST_STREAM:" + rstFrame.errorCode());
                                } else if (frame instanceof Http2HeadersFrame headersFrame) {
                                    String status = headersFrame.headers().status().toString();
                                    rejected.complete("STATUS:" + status);
                                }
                            }

                            @Override
                            public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
                                rejected.complete("EXCEPTION:" + cause.getClass().getSimpleName());
                            }

                            @Override
                            public void channelInactive(ChannelHandlerContext ctx) {
                                rejected.complete("CLOSED");
                            }
                        });

                // Plain CONNECT with :path (forbidden per RFC 9113 Section 8.5)
                Http2Headers headers = new DefaultHttp2Headers()
                        .method("CONNECT")
                        .authority(authority)
                        .path("/tunnel");

                stream.writeAndFlush(new DefaultHttp2HeadersFrame(headers, true)).sync();

                String result = rejected.get(10, TimeUnit.SECONDS);
                logger.info("H2 CONNECT+:path test result: {}", result);

                assertFalse(result.equals("STATUS:200"),
                        "CONNECT with :path must NOT succeed but got 200");
                assertTrue(result.startsWith("RST_STREAM") || result.startsWith("STATUS")
                                || result.startsWith("EXCEPTION") || result.equals("CLOSED"),
                        "CONNECT with :path must be rejected but got: " + result);

                parentChannel.close().sync();
            } finally {
                group.shutdownGracefully().sync();
            }
        }

        // =================================================================
        // H2 Helpers
        // =================================================================

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
            assertTrue(cf.isSuccess(), "H2 client must connect");

            Channel channel = cf.channel();
            channel.pipeline().get(SslHandler.class).handshakeFuture().sync();

            // Allow H2 SETTINGS exchange to complete
            Thread.sleep(300);

            return channel;
        }

        private Http2StreamChannel openStream(Channel parentChannel,
                                              SimpleChannelInboundHandler<Http2StreamFrame> handler) throws Exception {
            return new Http2StreamChannelBootstrap(parentChannel)
                    .handler(handler)
                    .open()
                    .sync()
                    .getNow();
        }
    }
}
