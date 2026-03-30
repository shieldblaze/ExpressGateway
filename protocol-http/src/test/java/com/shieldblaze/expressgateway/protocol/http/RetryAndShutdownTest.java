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

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.NodeBuilder;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterBuilder;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.HTTPRoundRobin;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.sessionpersistence.NOOPSessionPersistence;
import com.shieldblaze.expressgateway.common.utils.AvailablePortUtil;
import com.shieldblaze.expressgateway.configuration.ConfigurationContext;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStartupTask;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStopTask;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancer;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancerBuilder;
import com.shieldblaze.expressgateway.protocol.tcp.TCPListener;
import io.netty.buffer.Unpooled;
import io.netty.channel.ChannelHandler;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.SimpleChannelInboundHandler;
import io.netty.handler.codec.http.DefaultFullHttpResponse;
import io.netty.handler.codec.http.FullHttpRequest;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpVersion;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.MethodOrderer;
import org.junit.jupiter.api.Order;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.TestMethodOrder;
import org.junit.jupiter.api.Timeout;

import java.io.IOException;
import java.net.InetSocketAddress;
import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.nio.charset.StandardCharsets;
import java.time.Duration;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;

import static io.netty.handler.codec.http.HttpResponseStatus.OK;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * P1/P2 integration tests for retry logic, round-robin, node failover, and graceful shutdown.
 *
 * <p>Uses a single shared {@link HTTPLoadBalancer} instance and ordered test execution
 * to minimize resource pressure. Graceful shutdown tests run last since they stop the LB.</p>
 */
@Timeout(value = 120, unit = TimeUnit.SECONDS)
@TestMethodOrder(MethodOrderer.OrderAnnotation.class)
class RetryAndShutdownTest {

    private static int loadBalancerPort;
    private static HTTPLoadBalancer httpLoadBalancer;
    private static Cluster cluster;

    // Shared backend — started once, used by tests 1-4
    private static HttpServer sharedBackend;

    @BeforeAll
    static void setup() throws Exception {
        loadBalancerPort = AvailablePortUtil.getTcpPort();
        assertTrue(loadBalancerPort > 0, "Failed to allocate a free TCP port");

        cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new HTTPRoundRobin(NOOPSessionPersistence.INSTANCE))
                .build();

        httpLoadBalancer = HTTPLoadBalancerBuilder.newBuilder()
                .withConfigurationContext(ConfigurationContext.DEFAULT)
                .withBindAddress(new InetSocketAddress("127.0.0.1", loadBalancerPort))
                .withL4FrontListener(new TCPListener())
                .build();

        httpLoadBalancer.mappedCluster("127.0.0.1:" + loadBalancerPort, cluster);

        L4FrontListenerStartupTask startupTask = httpLoadBalancer.start();
        startupTask.future().get(60, TimeUnit.SECONDS);
        assertTrue(startupTask.isSuccess(), "Load balancer failed to start");

        // Start a shared backend for the basic tests
        sharedBackend = new HttpServer(false, new IdentifyingHandler("shared"));
        sharedBackend.start();
        sharedBackend.START_FUTURE.get(60, TimeUnit.SECONDS);
    }

    @AfterAll
    static void teardown() throws Exception {
        if (sharedBackend != null) {
            sharedBackend.shutdown();
            try { sharedBackend.SHUTDOWN_FUTURE.get(10, TimeUnit.SECONDS); } catch (Exception ignored) {}
        }
        if (httpLoadBalancer != null) {
            try { httpLoadBalancer.shutdown().future().get(10, TimeUnit.SECONDS); } catch (Exception ignored) {}
        }
    }

    private static HttpClient newHttpClient() {
        return HttpClient.newBuilder().connectTimeout(Duration.ofSeconds(5)).build();
    }

    private String sendRequest(String method, String path) throws Exception {
        HttpRequest.Builder builder = HttpRequest.newBuilder()
                .uri(URI.create("http://127.0.0.1:" + loadBalancerPort + path))
                .timeout(Duration.ofSeconds(10));

        switch (method) {
            case "GET" -> builder.GET();
            case "HEAD" -> builder.method("HEAD", HttpRequest.BodyPublishers.noBody());
            case "PUT" -> builder.PUT(HttpRequest.BodyPublishers.ofString("data"));
            case "DELETE" -> builder.DELETE();
            case "POST" -> builder.POST(HttpRequest.BodyPublishers.ofString("data"));
            default -> builder.method(method, HttpRequest.BodyPublishers.noBody());
        }

        HttpResponse<String> response = newHttpClient().send(builder.build(),
                HttpResponse.BodyHandlers.ofString());
        assertEquals(200, response.statusCode(),
                "Expected 200 OK for " + method + " " + path + " but got " + response.statusCode());
        return response.body();
    }

    private int sendRequestForStatus(String method, String path) throws Exception {
        HttpRequest.Builder builder = HttpRequest.newBuilder()
                .uri(URI.create("http://127.0.0.1:" + loadBalancerPort + path))
                .timeout(Duration.ofSeconds(10));

        switch (method) {
            case "GET" -> builder.GET();
            case "HEAD" -> builder.method("HEAD", HttpRequest.BodyPublishers.noBody());
            case "PUT" -> builder.PUT(HttpRequest.BodyPublishers.ofString("data"));
            case "DELETE" -> builder.DELETE();
            case "POST" -> builder.POST(HttpRequest.BodyPublishers.ofString("data"));
            default -> builder.method(method, HttpRequest.BodyPublishers.noBody());
        }

        HttpResponse<String> response = newHttpClient().send(builder.build(),
                HttpResponse.BodyHandlers.ofString());
        return response.statusCode();
    }

    // -----------------------------------------------------------------------
    //  P1: Idempotent Method Classification (unit-level, no backend needed)
    // -----------------------------------------------------------------------

    @Test
    @Order(1)
    void idempotentMethodClassification_perRfc9110() {
        assertTrue(UpstreamRetryHandler.isIdempotentMethod(io.netty.handler.codec.http.HttpMethod.GET));
        assertTrue(UpstreamRetryHandler.isIdempotentMethod(io.netty.handler.codec.http.HttpMethod.HEAD));
        assertTrue(UpstreamRetryHandler.isIdempotentMethod(io.netty.handler.codec.http.HttpMethod.PUT));
        assertTrue(UpstreamRetryHandler.isIdempotentMethod(io.netty.handler.codec.http.HttpMethod.DELETE));
        assertTrue(UpstreamRetryHandler.isIdempotentMethod(io.netty.handler.codec.http.HttpMethod.OPTIONS));
        assertTrue(UpstreamRetryHandler.isIdempotentMethod(io.netty.handler.codec.http.HttpMethod.TRACE));
        assertTrue(!UpstreamRetryHandler.isIdempotentMethod(io.netty.handler.codec.http.HttpMethod.POST));
        assertTrue(!UpstreamRetryHandler.isIdempotentMethod(io.netty.handler.codec.http.HttpMethod.PATCH));
        assertTrue(!UpstreamRetryHandler.isIdempotentMethod(io.netty.handler.codec.http.HttpMethod.CONNECT));
    }

    // -----------------------------------------------------------------------
    //  P1: Idempotent methods + POST succeed on healthy backend
    // -----------------------------------------------------------------------

    @Test
    @Order(2)
    void idempotentAndPostMethods_succeedWithHealthyBackend() throws Exception {
        NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress("127.0.0.1", sharedBackend.port()))
                .build();

        for (String method : java.util.List.of("GET", "PUT", "DELETE")) {
            String body = sendRequest(method, "/healthy-path");
            assertEquals("shared", body, method + " should succeed on healthy backend");
        }

        int headStatus = sendRequestForStatus("HEAD", "/healthy-path");
        assertEquals(200, headStatus, "HEAD should succeed");

        int postStatus = sendRequestForStatus("POST", "/post-path");
        assertEquals(200, postStatus, "POST should succeed on healthy backend");
    }

    // -----------------------------------------------------------------------
    //  P1: Single node offline — all methods fail
    // -----------------------------------------------------------------------

    @Test
    @Order(3)
    void singleNodeOffline_allMethodsFail() throws Exception {
        // Remove all existing nodes, add a dead one
        for (Node n : new java.util.ArrayList<>(cluster.allNodes())) {
            cluster.removeNode(n);
        }

        // Register a node pointing at a port with nothing listening
        int deadPort = AvailablePortUtil.getTcpPort();
        NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress("127.0.0.1", deadPort))
                .build();

        for (String method : java.util.List.of("GET", "POST")) {
            boolean failed = false;
            try {
                int status = sendRequestForStatus(method, "/dead-backend");
                if (status >= 500) failed = true;
            } catch (IOException e) {
                failed = true;
            }
            assertTrue(failed, method + " should fail when backend is unreachable");
        }
    }

    // -----------------------------------------------------------------------
    //  P2: Round-Robin Distribution
    // -----------------------------------------------------------------------

    @Test
    @Order(4)
    void roundRobin_distributesAcrossAllNodes() throws Exception {
        // Clear cluster and add 3 fresh backends
        for (Node n : new java.util.ArrayList<>(cluster.allNodes())) {
            cluster.removeNode(n);
        }

        HttpServer server1 = new HttpServer(false, new IdentifyingHandler("rr-1"));
        HttpServer server2 = new HttpServer(false, new IdentifyingHandler("rr-2"));
        HttpServer server3 = new HttpServer(false, new IdentifyingHandler("rr-3"));
        server1.start(); server1.START_FUTURE.get(60, TimeUnit.SECONDS);
        server2.start(); server2.START_FUTURE.get(60, TimeUnit.SECONDS);
        server3.start(); server3.START_FUTURE.get(60, TimeUnit.SECONDS);

        try {
            NodeBuilder.newBuilder().withCluster(cluster)
                    .withSocketAddress(new InetSocketAddress("127.0.0.1", server1.port())).build();
            NodeBuilder.newBuilder().withCluster(cluster)
                    .withSocketAddress(new InetSocketAddress("127.0.0.1", server2.port())).build();
            NodeBuilder.newBuilder().withCluster(cluster)
                    .withSocketAddress(new InetSocketAddress("127.0.0.1", server3.port())).build();

            // Brief pause to allow cluster state to settle after node changes
            Thread.sleep(500);

            Map<String, AtomicInteger> dist = new ConcurrentHashMap<>();
            for (int i = 0; i < 12; i++) {
                try {
                    String body = sendRequest("GET", "/round-robin");
                    dist.computeIfAbsent(body, k -> new AtomicInteger()).incrementAndGet();
                } catch (Exception e) {
                    // Retry once on transient failure (stale connection from previous test)
                    Thread.sleep(200);
                    String body = sendRequest("GET", "/round-robin");
                    dist.computeIfAbsent(body, k -> new AtomicInteger()).incrementAndGet();
                }
            }

            assertTrue(dist.containsKey("rr-1"), "rr-1 should serve traffic, got: " + dist);
            assertTrue(dist.containsKey("rr-2"), "rr-2 should serve traffic, got: " + dist);
            assertTrue(dist.containsKey("rr-3"), "rr-3 should serve traffic, got: " + dist);
        } finally {
            server1.shutdown(); server2.shutdown(); server3.shutdown();
            try { server1.SHUTDOWN_FUTURE.get(5, TimeUnit.SECONDS); } catch (Exception ignored) {}
            try { server2.SHUTDOWN_FUTURE.get(5, TimeUnit.SECONDS); } catch (Exception ignored) {}
            try { server3.SHUTDOWN_FUTURE.get(5, TimeUnit.SECONDS); } catch (Exception ignored) {}
        }
    }

    // -----------------------------------------------------------------------
    //  P2: Node Offline During Traffic
    // -----------------------------------------------------------------------

    @Test
    @Order(5)
    void nodeOfflineDuringTraffic_requestsRouteToRemainingNodes() throws Exception {
        // Clear cluster
        for (Node n : new java.util.ArrayList<>(cluster.allNodes())) {
            cluster.removeNode(n);
        }

        HttpServer serverA = new HttpServer(false, new IdentifyingHandler("node-A"));
        HttpServer serverB = new HttpServer(false, new IdentifyingHandler("node-B"));
        serverA.start(); serverA.START_FUTURE.get(60, TimeUnit.SECONDS);
        serverB.start(); serverB.START_FUTURE.get(60, TimeUnit.SECONDS);

        try {
            Node nodeA = NodeBuilder.newBuilder().withCluster(cluster)
                    .withSocketAddress(new InetSocketAddress("127.0.0.1", serverA.port())).build();
            NodeBuilder.newBuilder().withCluster(cluster)
                    .withSocketAddress(new InetSocketAddress("127.0.0.1", serverB.port())).build();

            // Phase 1: both backends serving
            Map<String, AtomicInteger> phase1 = new ConcurrentHashMap<>();
            for (int i = 0; i < 8; i++) {
                String body = sendRequest("GET", "/phase1");
                phase1.computeIfAbsent(body, k -> new AtomicInteger()).incrementAndGet();
            }
            assertTrue(phase1.containsKey("node-A"), "node-A should serve in phase 1");
            assertTrue(phase1.containsKey("node-B"), "node-B should serve in phase 1");

            // Take node-A offline
            nodeA.close();
            serverA.shutdown();
            try { serverA.SHUTDOWN_FUTURE.get(5, TimeUnit.SECONDS); } catch (Exception ignored) {}
            Thread.sleep(500);

            // Phase 2: only node-B should serve
            int successes = 0;
            for (int i = 0; i < 6; i++) {
                try {
                    String body = sendRequest("GET", "/phase2");
                    assertEquals("node-B", body, "Only node-B should serve after node-A removed");
                    successes++;
                } catch (Exception ignored) {}
            }
            assertTrue(successes >= 3, "At least half of phase 2 requests should succeed");
        } finally {
            try { serverB.shutdown(); serverB.SHUTDOWN_FUTURE.get(5, TimeUnit.SECONDS); } catch (Exception ignored) {}
        }
    }

    // -----------------------------------------------------------------------
    //  P1: Graceful Shutdown (runs LAST — stops the LB)
    // -----------------------------------------------------------------------

    @Test
    @Order(99)
    void gracefulShutdown_newConnectionsRejectedAfterStop() throws Exception {
        // Clear cluster and add shared backend
        for (Node n : new java.util.ArrayList<>(cluster.allNodes())) {
            cluster.removeNode(n);
        }
        NodeBuilder.newBuilder().withCluster(cluster)
                .withSocketAddress(new InetSocketAddress("127.0.0.1", sharedBackend.port())).build();

        // Verify LB works
        String body = sendRequest("GET", "/pre-shutdown");
        assertEquals("shared", body);

        // Stop the LB
        L4FrontListenerStopTask stopTask = httpLoadBalancer.stop();
        stopTask.future().get(30, TimeUnit.SECONDS);
        assertTrue(stopTask.isSuccess(), "Stop task should complete successfully");

        Thread.sleep(500);

        // New connections should be refused
        boolean connectionRefused = false;
        for (int attempt = 0; attempt < 3; attempt++) {
            try {
                HttpRequest request = HttpRequest.newBuilder()
                        .uri(URI.create("http://127.0.0.1:" + loadBalancerPort + "/post-shutdown"))
                        .timeout(Duration.ofSeconds(3)).GET().build();
                newHttpClient().send(request, HttpResponse.BodyHandlers.ofString());
            } catch (IOException e) {
                connectionRefused = true;
                break;
            }
        }

        assertTrue(connectionRefused, "New connections must be refused after LB stop");
        httpLoadBalancer = null; // Prevent @AfterAll from double-shutting down
    }

    // -----------------------------------------------------------------------
    //  Custom handler
    // -----------------------------------------------------------------------

    @ChannelHandler.Sharable
    private static final class IdentifyingHandler extends SimpleChannelInboundHandler<FullHttpRequest> {
        private final String serverName;

        IdentifyingHandler(String serverName) {
            this.serverName = serverName;
        }

        @Override
        protected void channelRead0(ChannelHandlerContext ctx, FullHttpRequest msg) {
            byte[] body = serverName.getBytes(StandardCharsets.UTF_8);
            DefaultFullHttpResponse response = new DefaultFullHttpResponse(
                    HttpVersion.HTTP_1_1, OK, Unpooled.wrappedBuffer(body));
            response.headers().set(HttpHeaderNames.CONTENT_LENGTH, body.length);
            response.headers().set(HttpHeaderNames.CONTENT_TYPE, "text/plain");
            response.headers().set(HttpHeaderNames.CONNECTION, "close");
            ctx.writeAndFlush(response);
        }
    }
}
