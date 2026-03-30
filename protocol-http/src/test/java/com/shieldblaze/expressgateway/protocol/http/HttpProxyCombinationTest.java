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
import io.netty.buffer.Unpooled;
import io.netty.channel.ChannelHandler;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.SimpleChannelInboundHandler;
import io.netty.handler.codec.http.DefaultFullHttpResponse;
import io.netty.handler.codec.http.FullHttpRequest;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpHeaderValues;
import io.netty.handler.codec.http.HttpVersion;
import io.netty.handler.codec.http2.HttpConversionUtil;
import io.netty.handler.ssl.util.InsecureTrustManagerFactory;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.Timeout;

import javax.net.ssl.SSLContext;
import java.io.Closeable;
import java.net.InetSocketAddress;
import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.nio.charset.StandardCharsets;
import java.security.SecureRandom;
import java.time.Duration;
import java.util.List;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;

import static io.netty.handler.codec.http.HttpResponseStatus.OK;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * P0 integration tests covering all four HTTP proxy protocol combinations:
 * <ul>
 *   <li>H1 frontend -> H1 backend (cleartext -> cleartext)</li>
 *   <li>H1 frontend -> H2 backend (cleartext -> TLS+ALPN, BUG-10 regression path)</li>
 *   <li>H2 frontend -> H1 backend (TLS+ALPN -> cleartext)</li>
 *   <li>H2 frontend -> H2 backend (TLS+ALPN -> TLS+ALPN)</li>
 * </ul>
 *
 * <p>Each combination is tested with both GET (simple request/response) and POST
 * (body echo verification, minimum 50 bytes). A separate test validates that
 * frontend keep-alive survives after the backend sends {@code Connection: close}
 * (BUG-14 regression).</p>
 *
 * <p>The test infrastructure uses dynamic port allocation ({@link AvailablePortUtil})
 * to avoid port conflicts. Each test method stands up its own load balancer and
 * backend server, then tears them down afterward. This allows all tests to be
 * independent and safe for parallel execution.</p>
 *
 * <p>Relevant RFCs:
 * <ul>
 *   <li>RFC 9110 (HTTP Semantics) -- request/response semantics, status codes</li>
 *   <li>RFC 9112 (HTTP/1.1) -- Connection header, keep-alive, Connection: close</li>
 *   <li>RFC 9113 (HTTP/2) -- ALPN negotiation, multiplexed streams</li>
 * </ul>
 */
@Timeout(value = 120, unit = TimeUnit.SECONDS)
@org.junit.jupiter.api.TestMethodOrder(org.junit.jupiter.api.MethodOrderer.OrderAnnotation.class)
class HttpProxyCombinationTest {

    /**
     * POST body payload -- 64 bytes, well above the 50-byte minimum.
     * Chosen to be ASCII-printable for easy debugging in failure messages.
     */
    private static final String POST_BODY =
            "ExpressGateway-integration-test-payload-64-bytes-exactly-padded!";

    static {
        // Sanity check: POST_BODY must be at least 50 bytes per requirement.
        if (POST_BODY.getBytes(StandardCharsets.UTF_8).length < 50) {
            throw new AssertionError("POST_BODY must be at least 50 bytes");
        }
    }

    // =========================================================================
    // H1 -> H1: cleartext frontend, cleartext backend
    // =========================================================================

    @Test @org.junit.jupiter.api.Order(1)
    void h1ToH1_GET() throws Exception {
        try (ProxyStack stack = ProxyStack.create(false, false, false)) {
            HttpResponse<String> response = sendGet(stack.lbPort, false, HttpClient.Version.HTTP_1_1);
            assertEquals(200, response.statusCode(), "H1->H1 GET must return 200");
            assertEquals("Meow", response.body(), "H1->H1 GET body must be 'Meow'");
        }
    }

    @Test @org.junit.jupiter.api.Order(2)
    void h1ToH1_POST() throws Exception {
        try (ProxyStack stack = ProxyStack.create(false, false, false, new EchoHandler())) {
            HttpResponse<String> response = sendPost(stack.lbPort, false, HttpClient.Version.HTTP_1_1, POST_BODY);
            assertEquals(200, response.statusCode(), "H1->H1 POST must return 200");
            assertEquals(POST_BODY, response.body(),
                    "H1->H1 POST response body must match the sent payload -- proxy must not corrupt data");
        }
    }

    // =========================================================================
    // H1 -> H2: cleartext frontend, TLS backend with ALPN H2
    // This is the BUG-10 path -- protocol translation from HTTP/1.1 to HTTP/2.
    // =========================================================================

    @Test @org.junit.jupiter.api.Order(7)
    // XPROTO-01/02: Cross-protocol tests enabled for v1.0.0 release
    void h1ToH2_GET() throws Exception {
        try (ProxyStack stack = ProxyStack.create(false, true, true)) {
            HttpResponse<String> response = sendGet(stack.lbPort, false, HttpClient.Version.HTTP_1_1);
            assertEquals(200, response.statusCode(), "H1->H2 GET must return 200");
            assertEquals("Meow", response.body(), "H1->H2 GET body must be 'Meow'");
        }
    }

    @Test @org.junit.jupiter.api.Order(8)
    // XPROTO-01/02: Cross-protocol tests enabled for v1.0.0 release
    void h1ToH2_POST() throws Exception {
        try (ProxyStack stack = ProxyStack.create(false, true, true, new EchoHandler())) {
            HttpResponse<String> response = sendPost(stack.lbPort, false, HttpClient.Version.HTTP_1_1, POST_BODY);
            assertEquals(200, response.statusCode(), "H1->H2 POST must return 200");
            assertEquals(POST_BODY, response.body(),
                    "H1->H2 POST body must survive protocol translation without corruption (BUG-10 path)");
        }
    }

    // =========================================================================
    // H2 -> H1: TLS frontend with ALPN H2, cleartext backend
    // =========================================================================

    @Test @org.junit.jupiter.api.Order(5)
    // XPROTO-01/02: Cross-protocol tests enabled for v1.0.0 release
    void h2ToH1_GET() throws Exception {
        try (ProxyStack stack = ProxyStack.create(true, false, false)) {
            HttpResponse<String> response = sendGet(stack.lbPort, true, HttpClient.Version.HTTP_2);
            assertEquals(200, response.statusCode(), "H2->H1 GET must return 200");
            assertEquals("Meow", response.body(), "H2->H1 GET body must be 'Meow'");
        }
    }

    @Test @org.junit.jupiter.api.Order(6)
    // XPROTO-01/02: Cross-protocol tests enabled for v1.0.0 release
    void h2ToH1_POST() throws Exception {
        try (ProxyStack stack = ProxyStack.create(true, false, false, new EchoHandler())) {
            HttpResponse<String> response = sendPost(stack.lbPort, true, HttpClient.Version.HTTP_2, POST_BODY);
            assertEquals(200, response.statusCode(), "H2->H1 POST must return 200");
            assertEquals(POST_BODY, response.body(),
                    "H2->H1 POST body must survive protocol downgrade without corruption");
        }
    }

    // =========================================================================
    // H2 -> H2: TLS frontend with ALPN H2, TLS backend with ALPN H2
    // =========================================================================

    @Test @org.junit.jupiter.api.Order(3)
    void h2ToH2_GET() throws Exception {
        try (ProxyStack stack = ProxyStack.create(true, true, true)) {
            HttpResponse<String> response = sendGet(stack.lbPort, true, HttpClient.Version.HTTP_2);
            assertEquals(200, response.statusCode(), "H2->H2 GET must return 200");
            assertEquals("Meow", response.body(), "H2->H2 GET body must be 'Meow'");
        }
    }

    @Test @org.junit.jupiter.api.Order(4)
    void h2ToH2_POST() throws Exception {
        try (ProxyStack stack = ProxyStack.create(true, true, true, new EchoHandler())) {
            HttpResponse<String> response = sendPost(stack.lbPort, true, HttpClient.Version.HTTP_2, POST_BODY);
            assertEquals(200, response.statusCode(), "H2->H2 POST must return 200");
            assertEquals(POST_BODY, response.body(),
                    "H2->H2 POST body must pass through end-to-end without corruption");
        }
    }

    // =========================================================================
    // BUG-14 regression: Backend Connection:close must not kill frontend
    // keep-alive. H1->H1 path, two sequential requests on the same HttpClient.
    //
    // Per RFC 9112 Section 9.6, a proxy receiving Connection: close from the
    // backend MUST close the backend connection but SHOULD keep the frontend
    // connection alive for subsequent requests. Before the BUG-14 fix, the
    // frontend channel was closed when the backend sent Connection: close,
    // breaking HTTP/1.1 keep-alive.
    // =========================================================================

    @Test @org.junit.jupiter.api.Order(9)
    void backendConnectionClose_frontendKeepAliveSurvives() throws Exception {
        ConnectionCloseHandler connectionCloseHandler = new ConnectionCloseHandler();
        try (ProxyStack stack = ProxyStack.create(false, false, false, connectionCloseHandler)) {
            // Use a single HttpClient instance so the JDK connection pool
            // reuses the same TCP connection for both requests.
            HttpClient client = newInsecureHttpClient();

            // First request -- backend will respond with Connection: close.
            HttpRequest request1 = HttpRequest.newBuilder()
                    .GET()
                    .uri(URI.create("http://localhost:" + stack.lbPort + "/first"))
                    .version(HttpClient.Version.HTTP_1_1)
                    .timeout(Duration.ofSeconds(30))
                    .build();

            HttpResponse<String> response1 = client.send(request1, HttpResponse.BodyHandlers.ofString());
            assertEquals(200, response1.statusCode(), "First request must succeed");
            assertEquals("Meow", response1.body(), "First request body must be 'Meow'");

            // Second request -- this must succeed even though the backend sent
            // Connection: close on the first response. The proxy should have
            // established a new backend connection transparently.
            HttpRequest request2 = HttpRequest.newBuilder()
                    .GET()
                    .uri(URI.create("http://localhost:" + stack.lbPort + "/second"))
                    .version(HttpClient.Version.HTTP_1_1)
                    .timeout(Duration.ofSeconds(30))
                    .build();

            HttpResponse<String> response2 = client.send(request2, HttpResponse.BodyHandlers.ofString());
            assertEquals(200, response2.statusCode(),
                    "Second request must succeed -- frontend keep-alive must survive backend Connection: close");
            assertEquals("Meow", response2.body(), "Second request body must be 'Meow'");

            // Verify both requests were served (the handler counts them).
            assertTrue(connectionCloseHandler.requestCount.get() >= 2,
                    "Backend must have received at least 2 requests, got: " + connectionCloseHandler.requestCount.get());
        }
    }

    // =========================================================================
    // Shared HTTP client and request helpers
    // =========================================================================

    /**
     * Creates a Java HttpClient that trusts all certificates (for self-signed
     * test certs) and follows redirects. A new client is created per call to
     * avoid connection pool interference between tests.
     */
    private static HttpClient newInsecureHttpClient() {
        try {
            SSLContext sslContext = SSLContext.getInstance("TLSv1.3");
            sslContext.init(null, InsecureTrustManagerFactory.INSTANCE.getTrustManagers(), new SecureRandom());
            return HttpClient.newBuilder()
                    .sslContext(sslContext)
                    .followRedirects(HttpClient.Redirect.ALWAYS)
                    .build();
        } catch (Exception ex) {
            throw new IllegalStateException("Failed to create insecure HttpClient", ex);
        }
    }

    /**
     * Sends a GET request to the load balancer at the given port.
     *
     * @param port    the load balancer listen port
     * @param useTls  true for HTTPS, false for HTTP
     * @param version HTTP_1_1 or HTTP_2
     * @return the response
     */
    private static HttpResponse<String> sendGet(int port, boolean useTls, HttpClient.Version version)
            throws Exception {
        String scheme = useTls ? "https" : "http";
        HttpRequest request = HttpRequest.newBuilder()
                .GET()
                .uri(URI.create(scheme + "://localhost:" + port + "/"))
                .version(version)
                .timeout(Duration.ofSeconds(30))
                .build();
        return newInsecureHttpClient().send(request, HttpResponse.BodyHandlers.ofString());
    }

    /**
     * Sends a POST request with the given body to the load balancer.
     *
     * @param port    the load balancer listen port
     * @param useTls  true for HTTPS, false for HTTP
     * @param version HTTP_1_1 or HTTP_2
     * @param body    the request body (UTF-8)
     * @return the response
     */
    private static HttpResponse<String> sendPost(int port, boolean useTls, HttpClient.Version version, String body)
            throws Exception {
        String scheme = useTls ? "https" : "http";
        HttpRequest request = HttpRequest.newBuilder()
                .POST(HttpRequest.BodyPublishers.ofString(body))
                .uri(URI.create(scheme + "://localhost:" + port + "/"))
                .version(version)
                .timeout(Duration.ofSeconds(30))
                .setHeader("Content-Type", "text/plain")
                .build();
        return newInsecureHttpClient().send(request, HttpResponse.BodyHandlers.ofString());
    }

    // =========================================================================
    // Custom Netty handlers for the backend HttpServer
    // =========================================================================

    /**
     * Echoes the request body back in the response. Used for POST tests to
     * verify the proxy does not corrupt, truncate, or drop the request body
     * during protocol translation.
     *
     * <p>Must be {@code @Sharable} because the same handler instance is added
     * to every child channel pipeline by {@link HttpServer}.</p>
     */
    @ChannelHandler.Sharable
    static final class EchoHandler extends SimpleChannelInboundHandler<FullHttpRequest> {

        @Override
        protected void channelRead0(ChannelHandlerContext ctx, FullHttpRequest msg) {
            byte[] body = new byte[msg.content().readableBytes()];
            msg.content().readBytes(body);

            DefaultFullHttpResponse response = new DefaultFullHttpResponse(
                    HttpVersion.HTTP_1_1, OK, Unpooled.wrappedBuffer(body));

            // For HTTP/2 streams, propagate the stream ID so the codec can
            // associate the response with the correct stream.
            if (msg.headers().contains(HttpConversionUtil.ExtensionHeaderNames.STREAM_ID.text())) {
                response.headers().set(
                        HttpConversionUtil.ExtensionHeaderNames.STREAM_ID.text(),
                        msg.headers().get(HttpConversionUtil.ExtensionHeaderNames.STREAM_ID.text()));
            } else {
                response.headers().set(HttpHeaderNames.CONTENT_LENGTH, body.length);
            }
            response.headers().set(HttpHeaderNames.CONTENT_TYPE, "text/plain");
            ctx.writeAndFlush(response);
        }
    }

    /**
     * Responds to every request with "Meow" and {@code Connection: close}.
     * Used for the BUG-14 regression test to verify the proxy correctly
     * handles a backend that closes its connection after each response without
     * propagating that closure to the frontend keep-alive connection.
     *
     * <p>Per RFC 9112 Section 9.6, Connection: close means the server will
     * close the connection after the response is sent. The proxy must handle
     * this gracefully by opening a new backend connection for the next
     * frontend request.</p>
     */
    @ChannelHandler.Sharable
    static final class ConnectionCloseHandler extends SimpleChannelInboundHandler<FullHttpRequest> {

        final AtomicInteger requestCount = new AtomicInteger(0);

        @Override
        protected void channelRead0(ChannelHandlerContext ctx, FullHttpRequest msg) {
            requestCount.incrementAndGet();

            byte[] body = "Meow".getBytes(StandardCharsets.UTF_8);
            DefaultFullHttpResponse response = new DefaultFullHttpResponse(
                    HttpVersion.HTTP_1_1, OK, Unpooled.wrappedBuffer(body));

            // Set Content-Length for HTTP/1.1 framing.
            response.headers().set(HttpHeaderNames.CONTENT_LENGTH, body.length);
            response.headers().set(HttpHeaderNames.CONTENT_TYPE, "text/html");

            // The critical header: instruct the proxy to close the backend
            // connection after this response.
            response.headers().set(HttpHeaderNames.CONNECTION, HttpHeaderValues.CLOSE);

            // For HTTP/2 streams, propagate the stream ID.
            if (msg.headers().contains(HttpConversionUtil.ExtensionHeaderNames.STREAM_ID.text())) {
                response.headers().set(
                        HttpConversionUtil.ExtensionHeaderNames.STREAM_ID.text(),
                        msg.headers().get(HttpConversionUtil.ExtensionHeaderNames.STREAM_ID.text()));
            }

            ctx.writeAndFlush(response);
        }
    }

    // =========================================================================
    // ProxyStack: encapsulates the full backend + load balancer lifecycle.
    //
    // Each test gets its own ProxyStack on a dynamic port, so tests are
    // independent and can run in parallel without port conflicts.
    // =========================================================================

    /**
     * A self-contained test fixture that starts an {@link HttpServer} backend
     * and an {@link HTTPLoadBalancer} frontend, wired together with a single
     * backend node. Implements {@link Closeable} so it can be used in
     * try-with-resources blocks.
     */
    private static final class ProxyStack implements Closeable {

        final int lbPort;
        private final HttpServer httpServer;
        private final HTTPLoadBalancer httpLoadBalancer;

        private ProxyStack(int lbPort, HttpServer httpServer, HTTPLoadBalancer httpLoadBalancer) {
            this.lbPort = lbPort;
            this.httpServer = httpServer;
            this.httpLoadBalancer = httpLoadBalancer;
        }

        /**
         * Creates a ProxyStack with the default "Meow" backend handler.
         */
        static ProxyStack create(boolean tlsServer, boolean tlsClient, boolean tlsBackend) throws Exception {
            return create(tlsServer, tlsClient, tlsBackend, null);
        }

        /**
         * Creates a ProxyStack with a custom backend handler.
         *
         * @param tlsServer  true to enable TLS+ALPN on the frontend (H2 clients)
         * @param tlsClient  true to enable TLS on the proxy-to-backend connection
         * @param tlsBackend true to enable TLS+ALPN on the backend server
         * @param handler    custom Netty handler for the backend, or null for default
         */
        static ProxyStack create(boolean tlsServer, boolean tlsClient, boolean tlsBackend,
                                 ChannelHandler handler) throws Exception {

            // --- Start the backend HttpServer on a random port ---
            HttpServer httpServer = handler != null
                    ? new HttpServer(tlsBackend, handler)
                    : new HttpServer(tlsBackend);
            httpServer.start();
            httpServer.START_FUTURE.get(60, TimeUnit.SECONDS);

            // --- Build the ConfigurationContext ---
            // For cleartext-only (no TLS anywhere), use DEFAULT which exactly
            // matches the pattern in RequestSmugglingTest/MalformedRequestTest.
            // For TLS paths, create isolated TLS configs to avoid mutating globals.
            ConfigurationContext configCtx;
            if (!tlsServer && !tlsClient) {
                configCtx = ConfigurationContext.DEFAULT;
            } else {
                SelfSignedCertificate ssc = SelfSignedCertificate.generateNew(
                        List.of("127.0.0.1"), List.of("localhost"));
                CertificateKeyPair certificateKeyPair = CertificateKeyPair.forClient(
                        List.of(ssc.x509Certificate()), ssc.keyPair().getPrivate());

                TlsClientConfiguration tlsClientConfiguration =
                        TlsClientConfiguration.copyFrom(TlsClientConfiguration.DEFAULT);
                TlsServerConfiguration tlsServerConfiguration =
                        TlsServerConfiguration.copyFrom(TlsServerConfiguration.DEFAULT);

                if (tlsServer) {
                    tlsServerConfiguration.enable();
                }
                tlsServerConfiguration.addMapping("localhost", certificateKeyPair);
                tlsServerConfiguration.defaultMapping(certificateKeyPair);

                if (tlsClient) {
                    tlsClientConfiguration.enable();
                    tlsClientConfiguration.setAcceptAllCerts(true);
                }
                tlsClientConfiguration.defaultMapping(CertificateKeyPair.newDefaultClientInstance());

                configCtx = ConfigurationContext.create(tlsClientConfiguration, tlsServerConfiguration);
            }

            // --- Build the load balancer on a random port ---
            int lbPort = AvailablePortUtil.getTcpPort();

            Cluster cluster = ClusterBuilder.newBuilder()
                    .withLoadBalance(new HTTPRoundRobin(NOOPSessionPersistence.INSTANCE))
                    .build();

            HTTPLoadBalancer httpLoadBalancer = HTTPLoadBalancerBuilder.newBuilder()
                    .withConfigurationContext(configCtx)
                    .withBindAddress(new InetSocketAddress("127.0.0.1", lbPort))
                    .build();

            httpLoadBalancer.mappedCluster("localhost:" + lbPort, cluster);

            NodeBuilder.newBuilder()
                    .withCluster(cluster)
                    .withSocketAddress(new InetSocketAddress("127.0.0.1", httpServer.port()))
                    .build();

            L4FrontListenerStartupTask startupTask = httpLoadBalancer.start();
            startupTask.future().get(60, TimeUnit.SECONDS);
            assertTrue(startupTask.isSuccess(),
                    "Load balancer must start successfully on port " + lbPort);

            // Brief pause to ensure the server socket is fully ready to accept
            // connections. Without this, the first test connection may arrive
            // before the Netty pipeline is fully initialized.
            Thread.sleep(200);

            return new ProxyStack(lbPort, httpServer, httpLoadBalancer);
        }

        @Override
        public void close() {
            // Shut down the load balancer first, then the backend, to avoid
            // in-flight requests hitting a dead backend during teardown.
            try {
                httpLoadBalancer.stop().future().get(30, TimeUnit.SECONDS);
            } catch (Exception e) {
                // Log but don't fail -- teardown errors should not mask test failures.
                System.err.println("Warning: load balancer stop failed: " + e.getMessage());
            }

            httpServer.shutdown();
            try {
                httpServer.SHUTDOWN_FUTURE.get(30, TimeUnit.SECONDS);
            } catch (Exception e) {
                System.err.println("Warning: backend server shutdown failed: " + e.getMessage());
            }
        }
    }
}
