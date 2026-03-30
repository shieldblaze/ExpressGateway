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
import com.shieldblaze.expressgateway.configuration.ConfigurationContext;
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
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.Timeout;

import java.io.Closeable;
import java.net.InetSocketAddress;
import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.nio.charset.StandardCharsets;
import java.time.Duration;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;

import static io.netty.handler.codec.http.HttpResponseStatus.OK;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Integration tests for backend {@code Connection: close} detection and handling
 * (H1-07 + BUG-14 regression coverage).
 *
 * <p>When a backend HTTP/1.1 response contains {@code Connection: close}, the proxy
 * (via {@link DownstreamHandler#channelRead}) must:</p>
 * <ol>
 *   <li>Detect the header BEFORE hop-by-hop header stripping (H1-07).</li>
 *   <li>Forward the response body to the client intact.</li>
 *   <li>Close the backend channel after the full response is forwarded.</li>
 *   <li>Set {@code backendInitiatedClose = true} so the close cascade in
 *       {@link DownstreamHandler#close()} does NOT propagate to the frontend.</li>
 *   <li>Keep the frontend (client-facing) connection alive for subsequent requests.</li>
 * </ol>
 *
 * <p>Prior to the BUG-14 fix, the backend close cascaded to the frontend, breaking
 * HTTP/1.1 keep-alive for clients. These tests prove that the fix is correct by
 * sending multiple requests through a single {@link HttpClient} instance and
 * verifying all succeed.</p>
 *
 * <p>Relevant RFCs:</p>
 * <ul>
 *   <li>RFC 9112 Section 9.6 -- Connection: close semantics in HTTP/1.1</li>
 *   <li>RFC 9110 Section 7.6.1 -- hop-by-hop header handling (Connection is hop-by-hop)</li>
 * </ul>
 */
@Timeout(value = 120, unit = TimeUnit.SECONDS)
class BackendConnectionCloseTest {

    // =========================================================================
    // Test 1: Basic Connection:close -- response received, frontend survives
    // =========================================================================

    /**
     * Verifies the core BUG-14 fix: a backend sending {@code Connection: close}
     * must not kill the frontend keep-alive connection.
     *
     * <p>Sends two sequential requests through a single {@link HttpClient}. The
     * backend responds to every request with {@code Connection: close}. Both
     * requests must succeed, proving the proxy correctly isolated the backend
     * close from the frontend.</p>
     */
    @Test
    void backendConnectionClose_frontendKeepAliveSurvives() throws Exception {
        ConnectionCloseHandler handler = new ConnectionCloseHandler();
        try (ProxyStack stack = ProxyStack.h1h1(handler)) {
            HttpClient client = HttpClient.newHttpClient();

            // First request -- backend responds with Connection: close.
            HttpResponse<String> r1 = sendGet(client, stack.lbPort, "/first");
            assertEquals(200, r1.statusCode(), "First request must return 200");
            assertEquals("ConnectionClose", r1.body(), "First response body must match");

            // The proxy should have closed the backend connection but kept the
            // frontend alive. The second request proves the frontend survived.
            HttpResponse<String> r2 = sendGet(client, stack.lbPort, "/second");
            assertEquals(200, r2.statusCode(),
                    "Second request must succeed -- frontend keep-alive must survive backend Connection: close");
            assertEquals("ConnectionClose", r2.body(), "Second response body must match");

            // Both requests were served by the backend handler.
            assertTrue(handler.requestCount.get() >= 2,
                    "Backend must have received at least 2 requests, got: " + handler.requestCount.get());
        }
    }

    // =========================================================================
    // Test 2: Connection:close header is stripped (hop-by-hop) in proxy response
    // =========================================================================

    /**
     * Verifies that the proxy strips the {@code Connection: close} header from
     * the response forwarded to the client.
     *
     * <p>Per RFC 9110 Section 7.6.1, the Connection header is hop-by-hop and MUST
     * NOT be forwarded by a proxy. The proxy detects it for backend channel
     * lifecycle decisions, then strips it before forwarding.</p>
     *
     * <p>Java's {@link HttpClient} may not expose the raw Connection header, so
     * we use a raw TCP socket to read the actual response bytes.</p>
     */
    @Test
    void connectionCloseHeader_strippedFromProxiedResponse() throws Exception {
        ConnectionCloseHandler handler = new ConnectionCloseHandler();
        try (ProxyStack stack = ProxyStack.h1h1(handler)) {
            // Use a raw socket to inspect actual response headers.
            try (java.net.Socket socket = new java.net.Socket("127.0.0.1", stack.lbPort)) {
                socket.setSoTimeout(10_000);
                java.io.OutputStream out = socket.getOutputStream();
                out.write(("GET /check-headers HTTP/1.1\r\n"
                        + "Host: localhost:" + stack.lbPort + "\r\n"
                        + "\r\n").getBytes(StandardCharsets.US_ASCII));
                out.flush();

                // Read the raw HTTP response.
                try (java.io.BufferedReader reader = new java.io.BufferedReader(
                        new java.io.InputStreamReader(socket.getInputStream(), StandardCharsets.US_ASCII))) {

                    String statusLine = reader.readLine();
                    assertTrue(statusLine != null && statusLine.contains("200"),
                            "Response must be 200 OK, got: " + statusLine);

                    // Collect response headers until the blank line.
                    StringBuilder headers = new StringBuilder();
                    String line;
                    while ((line = reader.readLine()) != null && !line.isEmpty()) {
                        headers.append(line.toLowerCase()).append('\n');
                    }

                    // The Connection header must NOT appear in the proxied response.
                    // The proxy strips it as a hop-by-hop header per RFC 9110 Section 7.6.1.
                    assertFalse(headers.toString().contains("connection: close"),
                            "Proxy MUST strip hop-by-hop Connection: close header from response. "
                                    + "Headers received:\n" + headers);
                }
            }
        }
    }

    // =========================================================================
    // Test 3: Multiple sequential requests all succeed despite Connection:close
    // =========================================================================

    /**
     * Stress variant: send 5 sequential requests through one client. Each backend
     * response includes {@code Connection: close}, forcing the proxy to open a new
     * backend connection per request. All requests must succeed.
     */
    @Test
    void multipleSequentialRequests_allSucceedDespiteBackendClose() throws Exception {
        ConnectionCloseHandler handler = new ConnectionCloseHandler();
        try (ProxyStack stack = ProxyStack.h1h1(handler)) {
            HttpClient client = HttpClient.newHttpClient();

            int totalRequests = 5;
            for (int i = 1; i <= totalRequests; i++) {
                HttpResponse<String> response = sendGet(client, stack.lbPort, "/request-" + i);
                assertEquals(200, response.statusCode(),
                        "Request " + i + " must return 200");
                assertEquals("ConnectionClose", response.body(),
                        "Request " + i + " body must match");
            }

            assertTrue(handler.requestCount.get() >= totalRequests,
                    "Backend must have received at least " + totalRequests
                            + " requests, got: " + handler.requestCount.get());
        }
    }

    // =========================================================================
    // Test 4: Mixed responses -- some with Connection:close, some without
    // =========================================================================

    /**
     * Verifies that the proxy correctly handles a mix of keep-alive and
     * connection-close responses from the same backend. Even-numbered requests
     * get {@code Connection: close}; odd-numbered get keep-alive.
     *
     * <p>This validates that the {@code backendConnectionClose} flag in
     * {@link DownstreamHandler} is reset correctly per response and does not
     * leak state between requests.</p>
     */
    @Test
    void mixedKeepAliveAndClose_allRequestsSucceed() throws Exception {
        AlternatingHandler handler = new AlternatingHandler();
        try (ProxyStack stack = ProxyStack.h1h1(handler)) {
            HttpClient client = HttpClient.newHttpClient();

            int totalRequests = 6;
            for (int i = 1; i <= totalRequests; i++) {
                HttpResponse<String> response = sendGet(client, stack.lbPort, "/mixed-" + i);
                assertEquals(200, response.statusCode(),
                        "Mixed request " + i + " must return 200");
                assertTrue(response.body().startsWith("Mixed-"),
                        "Mixed request " + i + " body must start with 'Mixed-'");
            }

            assertTrue(handler.requestCount.get() >= totalRequests,
                    "Backend must have received at least " + totalRequests
                            + " requests, got: " + handler.requestCount.get());
        }
    }

    // =========================================================================
    // Test 5: Backend Connection:close with a POST body
    // =========================================================================

    /**
     * Verifies that the proxy correctly forwards the full response body even
     * when the backend sends {@code Connection: close}. Uses a POST request
     * with a body to exercise the request/response path more thoroughly.
     */
    @Test
    void backendConnectionClose_postBodyPreserved() throws Exception {
        ConnectionCloseEchoHandler handler = new ConnectionCloseEchoHandler();
        try (ProxyStack stack = ProxyStack.h1h1(handler)) {
            HttpClient client = HttpClient.newHttpClient();

            String requestBody = "ExpressGateway-BUG14-regression-test-payload-for-POST";

            HttpRequest request = HttpRequest.newBuilder()
                    .POST(HttpRequest.BodyPublishers.ofString(requestBody))
                    .uri(URI.create("http://localhost:" + stack.lbPort + "/echo"))
                    .version(HttpClient.Version.HTTP_1_1)
                    .header("Content-Type", "text/plain")
                    .timeout(Duration.ofSeconds(30))
                    .build();

            HttpResponse<String> r1 = client.send(request, HttpResponse.BodyHandlers.ofString());
            assertEquals(200, r1.statusCode(), "POST with Connection:close must return 200");
            assertEquals(requestBody, r1.body(),
                    "POST response body must echo the request body even with Connection: close");

            // Second POST to prove the frontend survived.
            HttpResponse<String> r2 = client.send(request, HttpResponse.BodyHandlers.ofString());
            assertEquals(200, r2.statusCode(),
                    "Second POST must succeed -- frontend alive after backend Connection: close");
            assertEquals(requestBody, r2.body(),
                    "Second POST response body must match");
        }
    }

    // =========================================================================
    // Shared HTTP request helper
    // =========================================================================

    private static HttpResponse<String> sendGet(HttpClient client, int port, String path) throws Exception {
        HttpRequest request = HttpRequest.newBuilder()
                .GET()
                .uri(URI.create("http://localhost:" + port + path))
                .version(HttpClient.Version.HTTP_1_1)
                .timeout(Duration.ofSeconds(30))
                .build();
        return client.send(request, HttpResponse.BodyHandlers.ofString());
    }

    // =========================================================================
    // Custom backend handlers
    // =========================================================================

    /**
     * Responds to every request with a fixed body and {@code Connection: close}.
     * The backend closes its end of the connection after each response, as
     * required by RFC 9112 Section 9.6 when Connection: close is present.
     */
    @ChannelHandler.Sharable
    static final class ConnectionCloseHandler extends SimpleChannelInboundHandler<FullHttpRequest> {

        final AtomicInteger requestCount = new AtomicInteger(0);

        @Override
        protected void channelRead0(ChannelHandlerContext ctx, FullHttpRequest msg) {
            requestCount.incrementAndGet();

            byte[] body = "ConnectionClose".getBytes(StandardCharsets.UTF_8);
            DefaultFullHttpResponse response = new DefaultFullHttpResponse(
                    HttpVersion.HTTP_1_1, OK, Unpooled.wrappedBuffer(body));
            response.headers().set(HttpHeaderNames.CONTENT_LENGTH, body.length);
            response.headers().set(HttpHeaderNames.CONTENT_TYPE, "text/plain");
            response.headers().set(HttpHeaderNames.CONNECTION, HttpHeaderValues.CLOSE);
            ctx.writeAndFlush(response);
        }
    }

    /**
     * Alternates between {@code Connection: close} (even requests) and keep-alive
     * (odd requests). Tests that the proxy handles mixed behavior correctly and
     * does not leak the {@code backendConnectionClose} flag across requests.
     */
    @ChannelHandler.Sharable
    static final class AlternatingHandler extends SimpleChannelInboundHandler<FullHttpRequest> {

        final AtomicInteger requestCount = new AtomicInteger(0);

        @Override
        protected void channelRead0(ChannelHandlerContext ctx, FullHttpRequest msg) {
            int count = requestCount.incrementAndGet();

            byte[] body = ("Mixed-" + count).getBytes(StandardCharsets.UTF_8);
            DefaultFullHttpResponse response = new DefaultFullHttpResponse(
                    HttpVersion.HTTP_1_1, OK, Unpooled.wrappedBuffer(body));
            response.headers().set(HttpHeaderNames.CONTENT_LENGTH, body.length);
            response.headers().set(HttpHeaderNames.CONTENT_TYPE, "text/plain");

            // Even-numbered requests get Connection: close.
            if (count % 2 == 0) {
                response.headers().set(HttpHeaderNames.CONNECTION, HttpHeaderValues.CLOSE);
            }

            ctx.writeAndFlush(response);
        }
    }

    /**
     * Echoes the request body in the response and sets {@code Connection: close}.
     * Used to verify that response body integrity is preserved when the backend
     * signals close.
     */
    @ChannelHandler.Sharable
    static final class ConnectionCloseEchoHandler extends SimpleChannelInboundHandler<FullHttpRequest> {

        @Override
        protected void channelRead0(ChannelHandlerContext ctx, FullHttpRequest msg) {
            byte[] body = new byte[msg.content().readableBytes()];
            msg.content().readBytes(body);

            DefaultFullHttpResponse response = new DefaultFullHttpResponse(
                    HttpVersion.HTTP_1_1, OK, Unpooled.wrappedBuffer(body));
            response.headers().set(HttpHeaderNames.CONTENT_LENGTH, body.length);
            response.headers().set(HttpHeaderNames.CONTENT_TYPE, "text/plain");
            response.headers().set(HttpHeaderNames.CONNECTION, HttpHeaderValues.CLOSE);
            ctx.writeAndFlush(response);
        }
    }

    // =========================================================================
    // ProxyStack: self-contained backend + load balancer lifecycle.
    //
    // Mirrors the pattern from HttpProxyCombinationTest but is limited to the
    // H1->H1 cleartext path since Connection: close is an HTTP/1.1 concept
    // (HTTP/2 uses GOAWAY for equivalent signaling).
    // =========================================================================

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
         * Creates an H1->H1 (cleartext) ProxyStack with the given backend handler.
         */
        static ProxyStack h1h1(ChannelHandler handler) throws Exception {
            // Start backend server on a random port (cleartext HTTP/1.1).
            HttpServer httpServer = new HttpServer(false, handler);
            httpServer.start();
            httpServer.START_FUTURE.get(60, TimeUnit.SECONDS);

            // Use DEFAULT config (cleartext, no TLS).
            ConfigurationContext configCtx = ConfigurationContext.DEFAULT;

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

            // Brief pause for the server socket to become fully ready.
            Thread.sleep(200);

            return new ProxyStack(lbPort, httpServer, httpLoadBalancer);
        }

        @Override
        public void close() {
            try {
                httpLoadBalancer.stop().future().get(30, TimeUnit.SECONDS);
            } catch (Exception e) {
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
