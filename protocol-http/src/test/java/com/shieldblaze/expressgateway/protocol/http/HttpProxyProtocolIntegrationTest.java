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
import com.shieldblaze.expressgateway.configuration.transport.BackendProxyProtocolMode;
import com.shieldblaze.expressgateway.configuration.transport.ProxyProtocolMode;
import com.shieldblaze.expressgateway.configuration.transport.ReceiveBufferAllocationType;
import com.shieldblaze.expressgateway.configuration.transport.TransportConfiguration;
import com.shieldblaze.expressgateway.configuration.transport.TransportType;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStartupTask;
import com.shieldblaze.expressgateway.core.handlers.ProxyProtocolHandler;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancer;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancerBuilder;
import io.netty.bootstrap.ServerBootstrap;
import io.netty.buffer.Unpooled;
import io.netty.channel.ChannelFuture;
import io.netty.channel.ChannelHandler;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInitializer;
import io.netty.channel.EventLoopGroup;
import io.netty.channel.SimpleChannelInboundHandler;
import io.netty.channel.nio.NioEventLoopGroup;
import io.netty.channel.socket.SocketChannel;
import io.netty.channel.socket.nio.NioServerSocketChannel;
import io.netty.handler.codec.http.DefaultFullHttpResponse;
import io.netty.handler.codec.http.FullHttpRequest;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpObjectAggregator;
import io.netty.handler.codec.http.HttpResponseStatus;
import io.netty.handler.codec.http.HttpServerCodec;
import io.netty.handler.codec.http.HttpVersion;
import org.junit.jupiter.api.MethodOrderer;
import org.junit.jupiter.api.Order;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.TestMethodOrder;
import org.junit.jupiter.api.Timeout;

import java.io.Closeable;
import java.net.InetSocketAddress;
import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.nio.charset.StandardCharsets;
import java.time.Duration;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.TimeUnit;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * End-to-end HTTP integration tests for outbound PROXY protocol encoding.
 *
 * <p>Each test runs a custom Netty HTTP backend whose pipeline is:
 * <pre>
 *   ProxyProtocolHandler (AUTO) -> HttpServerCodec -> HttpObjectAggregator -> RequestHandler
 * </pre>
 * The {@link ProxyProtocolHandler} decodes the PROXY header sent by the proxy,
 * stores the real client address as a channel attribute, and removes itself.
 * The {@code RequestHandler} captures the address and responds with the decoded
 * source IP in the response body.</p>
 *
 * <p>Test cases cover:
 * <ul>
 *   <li>H1 backend receives PROXY v1 header then an HTTP GET</li>
 *   <li>H1 backend receives PROXY v2 header then an HTTP GET</li>
 *   <li>PROXY v1 with POST body echo — data must survive the PP header injection</li>
 *   <li>Response reaches the client correctly end-to-end</li>
 * </ul>
 */
@Timeout(value = 60, unit = TimeUnit.SECONDS)
@TestMethodOrder(MethodOrderer.OrderAnnotation.class)
class HttpProxyProtocolIntegrationTest {

    // =========================================================================
    // Test 1: H1 backend receives PROXY v1 header then HTTP GET
    // =========================================================================

    /**
     * Verifies that when {@code backendProxyProtocolMode=V1}, the proxy prefixes the
     * backend connection with a PROXY v1 text header. The backend (via
     * {@link ProxyProtocolHandler} AUTO mode) decodes it, sets the channel attribute,
     * and the {@link AddressEchoHandler} writes the decoded source IP into the response.
     *
     * <p>Because the test client connects through localhost, the source IP in the
     * header must be {@code 127.0.0.1}.</p>
     */
    @Order(1)
    @Test
    void h1BackendReceivesV1HeaderOnGet() throws Exception {
        try (ProxyStack stack = ProxyStack.create(BackendProxyProtocolMode.V1)) {
            HttpResponse<String> response = sendGet(stack.lbPort());
            assertEquals(200, response.statusCode(), "H1->PP-v1 GET must return 200");
            assertEquals("127.0.0.1", response.body(),
                    "Response body must contain the client source IP decoded from the PROXY v1 header");
        }
    }

    // =========================================================================
    // Test 2: H1 backend receives PROXY v2 header then HTTP GET
    // =========================================================================

    /**
     * Same as {@link #h1BackendReceivesV1HeaderOnGet} but with PROXY protocol v2
     * (binary). The 12-byte signature plus address fields must be decoded correctly
     * by the backend's {@link ProxyProtocolHandler}.
     */
    @Order(2)
    @Test
    void h1BackendReceivesV2HeaderOnGet() throws Exception {
        try (ProxyStack stack = ProxyStack.create(BackendProxyProtocolMode.V2)) {
            HttpResponse<String> response = sendGet(stack.lbPort());
            assertEquals(200, response.statusCode(), "H1->PP-v2 GET must return 200");
            assertEquals("127.0.0.1", response.body(),
                    "Response body must contain the client source IP decoded from the PROXY v2 header");
        }
    }

    // =========================================================================
    // Test 3: PROXY v1 with POST body — data integrity after PP header injection
    // =========================================================================

    /**
     * Verifies that injecting a PROXY v1 header does not corrupt the subsequent
     * HTTP request body. The backend is configured to echo the POST body back.
     * The proxy inserts the PP header as raw bytes before the HTTP request;
     * the backend's {@link ProxyProtocolHandler} consumes exactly those bytes
     * and then the HTTP codec processes the request normally.
     */
    @Order(3)
    @Test
    void v1HeaderDoesNotCorruptHttpPostBody() throws Exception {
        String postBody = "ExpressGateway-PP-v1-POST-body-integrity-check-payload-64b!!!!";
        try (ProxyStack stack = ProxyStack.create(BackendProxyProtocolMode.V1)) {
            HttpResponse<String> response = sendPost(stack.lbPort(), postBody);
            assertEquals(200, response.statusCode(), "POST with PROXY v1 must return 200");
            assertEquals(postBody, response.body(),
                    "POST body must pass through the proxy unchanged after PROXY v1 header injection");
        }
    }

    // =========================================================================
    // Test 4: PROXY v2 with POST body — data integrity check
    // =========================================================================

    @Order(4)
    @Test
    void v2HeaderDoesNotCorruptHttpPostBody() throws Exception {
        String postBody = "ExpressGateway-PP-v2-POST-body-integrity-check-payload-64b!!!!";
        try (ProxyStack stack = ProxyStack.create(BackendProxyProtocolMode.V2)) {
            HttpResponse<String> response = sendPost(stack.lbPort(), postBody);
            assertEquals(200, response.statusCode(), "POST with PROXY v2 must return 200");
            assertEquals(postBody, response.body(),
                    "POST body must pass through the proxy unchanged after PROXY v2 header injection");
        }
    }

    // =========================================================================
    // Test 5: OFF mode — no PP header, plain HTTP still works
    // =========================================================================

    /**
     * Baseline: with {@code backendProxyProtocolMode=OFF}, no PROXY protocol header
     * is injected. The backend uses a plain HTTP pipeline (no PP handler).
     * HTTP requests must still succeed.
     */
    @Order(5)
    @Test
    void offModeNoHeaderPlainHttpWorks() throws Exception {
        try (PlainHttpStack stack = PlainHttpStack.create()) {
            HttpResponse<String> response = sendGet(stack.lbPort());
            assertEquals(200, response.statusCode(), "H1 GET must return 200 with OFF mode");
            assertEquals("Meow", response.body(), "Default handler must return 'Meow'");
        }
    }

    // =========================================================================
    // Shared HTTP client helpers
    // =========================================================================

    private static HttpResponse<String> sendGet(int lbPort) throws Exception {
        HttpRequest request = HttpRequest.newBuilder()
                .GET()
                .uri(URI.create("http://127.0.0.1:" + lbPort + "/"))
                .version(HttpClient.Version.HTTP_1_1)
                .timeout(Duration.ofSeconds(15))
                .build();
        return HttpClient.newHttpClient().send(request, HttpResponse.BodyHandlers.ofString());
    }

    private static HttpResponse<String> sendPost(int lbPort, String body) throws Exception {
        HttpRequest request = HttpRequest.newBuilder()
                .POST(HttpRequest.BodyPublishers.ofString(body))
                .uri(URI.create("http://127.0.0.1:" + lbPort + "/"))
                .version(HttpClient.Version.HTTP_1_1)
                .timeout(Duration.ofSeconds(15))
                .header("Content-Type", "text/plain")
                .build();
        return HttpClient.newHttpClient().send(request, HttpResponse.BodyHandlers.ofString());
    }

    // =========================================================================
    // ProxyStack: backend with PP decoder + HTTP codec, LB with PP encoder
    // =========================================================================

    /**
     * Test fixture: a Netty HTTP backend that decodes a PROXY protocol header and
     * an {@link HTTPLoadBalancer} configured with the given PP mode, wired together.
     *
     * <p>Backend pipeline:
     * <pre>
     *   ProxyProtocolHandler (AUTO) -> HttpServerCodec -> HttpObjectAggregator -> handler
     * </pre>
     * The handler used depends on the request type:
     * <ul>
     *   <li>GET: {@link AddressEchoHandler} — returns the decoded source IP as the body</li>
     *   <li>POST: {@link EchoBodyHandler} — returns the request body unchanged</li>
     * </ul>
     * Since different tests need different handlers and the HTTP server is not
     * restarted per test, the backend uses a {@link DispatchHandler} that routes
     * to either handler based on the HTTP method.
     * </p>
     */
    private static final class ProxyStack implements Closeable {

        private final int lbPort;
        private final PpAwareHttpBackend backend;
        private final HTTPLoadBalancer lb;

        private ProxyStack(int lbPort, PpAwareHttpBackend backend, HTTPLoadBalancer lb) {
            this.lbPort = lbPort;
            this.backend = backend;
            this.lb = lb;
        }

        int lbPort() {
            return lbPort;
        }

        static ProxyStack create(BackendProxyProtocolMode ppMode) throws Exception {
            PpAwareHttpBackend backend = new PpAwareHttpBackend();
            backend.start();

            TransportConfiguration transportConfig = new TransportConfiguration()
                    .transportType(TransportType.NIO)
                    .receiveBufferAllocationType(ReceiveBufferAllocationType.ADAPTIVE)
                    .receiveBufferSizes(new int[]{512, 9001, 65535})
                    .tcpConnectionBacklog(1024)
                    .socketReceiveBufferSize(65536)
                    .socketSendBufferSize(65536)
                    .tcpFastOpenMaximumPendingRequests(100)
                    .backendConnectTimeout(5_000)
                    .connectionIdleTimeout(60_000)
                    .proxyProtocolMode(ProxyProtocolMode.OFF)
                    .backendProxyProtocolMode(ppMode)
                    .validate();

            ConfigurationContext configCtx = ConfigurationContext.create(transportConfig);

            int lbPort = AvailablePortUtil.getTcpPort();

            Cluster cluster = ClusterBuilder.newBuilder()
                    .withLoadBalance(new HTTPRoundRobin(NOOPSessionPersistence.INSTANCE))
                    .build();

            HTTPLoadBalancer lb = HTTPLoadBalancerBuilder.newBuilder()
                    .withConfigurationContext(configCtx)
                    .withBindAddress(new InetSocketAddress("127.0.0.1", lbPort))
                    .build();

            lb.mappedCluster("DEFAULT", cluster);

            NodeBuilder.newBuilder()
                    .withCluster(cluster)
                    .withSocketAddress(new InetSocketAddress("127.0.0.1", backend.port()))
                    .build();

            L4FrontListenerStartupTask startup = lb.start();
            startup.future().get(10, TimeUnit.SECONDS);
            assertTrue(startup.isSuccess(), "HTTP LB must start for PP mode " + ppMode);

            // Allow the server socket to fully initialize before accepting connections
            Thread.sleep(100);

            return new ProxyStack(lbPort, backend, lb);
        }

        @Override
        public void close() {
            try {
                lb.stop().future().get(15, TimeUnit.SECONDS);
            } catch (Exception e) {
                System.err.println("Warning: LB stop failed: " + e.getMessage());
            }
            backend.shutdown();
        }
    }

    // =========================================================================
    // PlainHttpStack: plain HTTP backend + LB with OFF mode (baseline)
    // =========================================================================

    private static final class PlainHttpStack implements Closeable {

        private final int lbPort;
        private final HttpServer httpServer;
        private final HTTPLoadBalancer lb;

        private PlainHttpStack(int lbPort, HttpServer httpServer, HTTPLoadBalancer lb) {
            this.lbPort = lbPort;
            this.httpServer = httpServer;
            this.lb = lb;
        }

        int lbPort() {
            return lbPort;
        }

        static PlainHttpStack create() throws Exception {
            HttpServer httpServer = new HttpServer(false);
            httpServer.start();
            httpServer.START_FUTURE.get(10, TimeUnit.SECONDS);

            int lbPort = AvailablePortUtil.getTcpPort();

            Cluster cluster = ClusterBuilder.newBuilder()
                    .withLoadBalance(new HTTPRoundRobin(NOOPSessionPersistence.INSTANCE))
                    .build();

            HTTPLoadBalancer lb = HTTPLoadBalancerBuilder.newBuilder()
                    .withConfigurationContext(ConfigurationContext.DEFAULT)
                    .withBindAddress(new InetSocketAddress("127.0.0.1", lbPort))
                    .build();

            lb.mappedCluster("DEFAULT", cluster);

            NodeBuilder.newBuilder()
                    .withCluster(cluster)
                    .withSocketAddress(new InetSocketAddress("127.0.0.1", httpServer.port()))
                    .build();

            L4FrontListenerStartupTask startup = lb.start();
            startup.future().get(10, TimeUnit.SECONDS);
            assertTrue(startup.isSuccess(), "Plain HTTP LB must start");

            Thread.sleep(100);
            return new PlainHttpStack(lbPort, httpServer, lb);
        }

        @Override
        public void close() {
            try {
                lb.stop().future().get(15, TimeUnit.SECONDS);
            } catch (Exception e) {
                System.err.println("Warning: LB stop failed: " + e.getMessage());
            }
            httpServer.shutdown();
        }
    }

    // =========================================================================
    // PpAwareHttpBackend: Netty server with PP decoder + HTTP codec
    // =========================================================================

    /**
     * Netty HTTP backend with pipeline:
     * <pre>
     *   ProxyProtocolHandler (AUTO) -> HttpServerCodec -> HttpObjectAggregator -> DispatchHandler
     * </pre>
     *
     * <p>The {@link DispatchHandler} routes:
     * <ul>
     *   <li>GET requests to {@link AddressEchoHandler}: returns the decoded source IP</li>
     *   <li>POST requests to {@link EchoBodyHandler}: returns the request body verbatim</li>
     * </ul>
     */
    private static final class PpAwareHttpBackend {

        private EventLoopGroup group;
        private ChannelFuture serverFuture;
        private int port;

        int port() {
            return port;
        }

        void start() throws Exception {
            group = new NioEventLoopGroup(1);
            CompletableFuture<Void> bindFuture = new CompletableFuture<>();

            serverFuture = new ServerBootstrap()
                    .group(group)
                    .channel(NioServerSocketChannel.class)
                    .childHandler(new ChannelInitializer<SocketChannel>() {
                        @Override
                        protected void initChannel(SocketChannel ch) {
                            ch.pipeline().addLast(new ProxyProtocolHandler(ProxyProtocolMode.AUTO));
                            ch.pipeline().addLast(new HttpServerCodec());
                            ch.pipeline().addLast(new HttpObjectAggregator(1024 * 1024));
                            ch.pipeline().addLast(new DispatchHandler());
                        }
                    })
                    .bind("127.0.0.1", 0)
                    .addListener(f -> {
                        if (f.isSuccess()) {
                            port = ((InetSocketAddress) serverFuture.channel().localAddress()).getPort();
                            bindFuture.complete(null);
                        } else {
                            bindFuture.completeExceptionally(f.cause());
                        }
                    });

            bindFuture.get(5, TimeUnit.SECONDS);
        }

        void shutdown() {
            if (serverFuture != null) {
                serverFuture.channel().close();
            }
            if (group != null) {
                group.shutdownGracefully(0, 1, TimeUnit.SECONDS);
            }
        }
    }

    // =========================================================================
    // Backend channel handlers
    // =========================================================================

    /**
     * Routes GET to {@link AddressEchoHandler} and POST to {@link EchoBodyHandler}.
     * Must be {@code @Sharable} because the same instance is reused across child channels.
     */
    @ChannelHandler.Sharable
    private static final class DispatchHandler extends SimpleChannelInboundHandler<FullHttpRequest> {

        private final AddressEchoHandler addressEchoHandler = new AddressEchoHandler();
        private final EchoBodyHandler echoBodyHandler = new EchoBodyHandler();

        @Override
        protected void channelRead0(ChannelHandlerContext ctx, FullHttpRequest request) throws Exception {
            if ("GET".equalsIgnoreCase(request.method().name())) {
                addressEchoHandler.channelRead0(ctx, request);
            } else {
                echoBodyHandler.channelRead0(ctx, request);
            }
        }
    }

    /**
     * Returns the source IP decoded from the PROXY protocol header as the HTTP response body.
     * If no address was decoded (e.g., UNKNOWN / LOCAL command), returns "UNKNOWN".
     */
    @ChannelHandler.Sharable
    private static final class AddressEchoHandler extends SimpleChannelInboundHandler<FullHttpRequest> {

        @Override
        protected void channelRead0(ChannelHandlerContext ctx, FullHttpRequest request) {
            InetSocketAddress realAddr = ctx.channel().attr(ProxyProtocolHandler.REAL_CLIENT_ADDRESS).get();
            String body = (realAddr != null)
                    ? realAddr.getAddress().getHostAddress()
                    : "UNKNOWN";

            byte[] bodyBytes = body.getBytes(StandardCharsets.UTF_8);
            DefaultFullHttpResponse response = new DefaultFullHttpResponse(
                    HttpVersion.HTTP_1_1,
                    HttpResponseStatus.OK,
                    Unpooled.wrappedBuffer(bodyBytes));
            response.headers().set(HttpHeaderNames.CONTENT_LENGTH, bodyBytes.length);
            response.headers().set(HttpHeaderNames.CONTENT_TYPE, "text/plain");
            ctx.writeAndFlush(response);
        }
    }

    /**
     * Echoes the HTTP request body back as the response body. Used to verify that
     * PP header injection does not corrupt subsequent application data.
     */
    @ChannelHandler.Sharable
    private static final class EchoBodyHandler extends SimpleChannelInboundHandler<FullHttpRequest> {

        @Override
        protected void channelRead0(ChannelHandlerContext ctx, FullHttpRequest request) {
            byte[] body = new byte[request.content().readableBytes()];
            request.content().readBytes(body);

            DefaultFullHttpResponse response = new DefaultFullHttpResponse(
                    HttpVersion.HTTP_1_1,
                    HttpResponseStatus.OK,
                    Unpooled.wrappedBuffer(body));
            response.headers().set(HttpHeaderNames.CONTENT_LENGTH, body.length);
            response.headers().set(HttpHeaderNames.CONTENT_TYPE, "text/plain");
            ctx.writeAndFlush(response);
        }
    }
}
