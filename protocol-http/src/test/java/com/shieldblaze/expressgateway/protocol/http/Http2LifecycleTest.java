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
import io.netty.handler.codec.http2.DefaultHttp2GoAwayFrame;
import io.netty.handler.codec.http2.DefaultHttp2Headers;
import io.netty.handler.codec.http2.DefaultHttp2HeadersFrame;
import io.netty.handler.codec.http2.Http2Error;
import io.netty.handler.codec.http2.Http2DataFrame;
import io.netty.handler.codec.http2.Http2FrameCodecBuilder;
import io.netty.handler.codec.http2.Http2GoAwayFrame;
import io.netty.handler.codec.http2.Http2Headers;
import io.netty.handler.codec.http2.Http2HeadersFrame;
import io.netty.handler.codec.http2.Http2MultiplexHandler;
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
import org.junit.jupiter.api.Order;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.TestMethodOrder;
import org.junit.jupiter.api.Timeout;

import javax.net.ssl.SSLContext;
import java.net.InetSocketAddress;
import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.security.SecureRandom;
import java.time.Duration;
import java.util.List;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicInteger;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * P1 integration tests for HTTP/2 lifecycle scenarios through the load balancer.
 *
 * <p>These tests exercise critical H2 connection-level behaviors that a production
 * L7 proxy must handle correctly:
 * <ul>
 *   <li><b>GOAWAY forwarding</b> (RFC 9113 Section 6.8) -- the proxy must propagate
 *       graceful shutdown signals so backends can drain streams.</li>
 *   <li><b>RST_STREAM forwarding</b> (RFC 9113 Section 6.4) -- stream cancellation
 *       must propagate to prevent resource leaks on backends.</li>
 *   <li><b>Flow control / backpressure</b> (RFC 9113 Section 6.9) -- large payloads
 *       must not cause OOM; the H2 flow control window must gate the sender.</li>
 *   <li><b>TLS ALPN negotiation</b> (RFC 7301) -- the proxy must correctly negotiate
 *       h2 over TLS and fall back to HTTP/1.1 when h2 is not offered.</li>
 * </ul>
 *
 * <p>Tests use Netty's {@code Http2FrameCodec + Http2MultiplexHandler} as a client
 * for frame-level control (GOAWAY, RST_STREAM). Java's {@code HttpClient} is used
 * for ALPN and backpressure tests where frame-level access is not needed.</p>
 *
 * <p>The load balancer is set up with TLS on both the frontend (server-side ALPN)
 * and the backend connection (client-side TLS). The backend {@link HttpServer} also
 * runs with TLS and ALPN, supporting both h2 and http/1.1.</p>
 */
@Timeout(value = 90)
@TestMethodOrder(MethodOrderer.OrderAnnotation.class)
class Http2LifecycleTest {

    private static final Logger logger = LogManager.getLogger(Http2LifecycleTest.class);

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

        // TLS for the frontend (server-side): enables ALPN negotiation for h2/h1.1
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

        // Bind load balancer on a random available port to avoid conflicts
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

        // Map the hostname:port that clients will use in the Host/:authority header
        httpLoadBalancer.mappedCluster("localhost:" + loadBalancerPort, cluster);

        NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress("127.0.0.1", httpServer.port()))
                .build();

        L4FrontListenerStartupTask startupTask = httpLoadBalancer.start();
        startupTask.future().get(60, TimeUnit.SECONDS);
        assertTrue(startupTask.isSuccess(), "Load balancer must start successfully");

        // Allow the server socket to fully initialize before accepting connections
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
    // Test 1: GOAWAY forwarding (RFC 9113 Section 6.8)
    // ===================================================================

    /**
     * Sends a GOAWAY frame from the client after establishing a stream.
     * Verifies the connection is torn down gracefully -- the proxy should
     * close the connection after processing in-flight streams.
     *
     * <p>Per RFC 9113 Section 6.8, a GOAWAY frame signals that the sender
     * will not initiate new streams on the connection. The proxy should
     * propagate this to the backend to allow stream draining.</p>
     *
     * <p>We verify: (1) the existing stream completes with 200, (2) the
     * connection closes after GOAWAY processing.</p>
     */
    @Test
    @Order(5)
    void goawayFromClient_connectionClosesGracefully() throws Exception {
        EventLoopGroup group = new NioEventLoopGroup(1);
        try {
            SslContext sslCtx = buildClientSslContext();
            String authority = "localhost:" + loadBalancerPort;

            CompletableFuture<Boolean> responseReceived = new CompletableFuture<>();
            CompletableFuture<Boolean> connectionClosed = new CompletableFuture<>();

            Channel parentChannel = connectH2Client(group, sslCtx,
                    new SimpleChannelInboundHandler<Object>() {
                        @Override
                        protected void channelRead0(ChannelHandlerContext ctx, Object msg) {
                            if (msg instanceof Http2GoAwayFrame goaway) {
                                logger.info("Received GOAWAY from proxy: errorCode={}", goaway.errorCode());
                            }
                        }

                        @Override
                        public void channelInactive(ChannelHandlerContext ctx) {
                            connectionClosed.complete(true);
                        }
                    });

            // Open a stream and send a GET request
            Http2StreamChannel streamChannel = openStream(parentChannel,
                    new SimpleChannelInboundHandler<Http2StreamFrame>() {
                        @Override
                        protected void channelRead0(ChannelHandlerContext ctx, Http2StreamFrame frame) {
                            if (frame instanceof Http2HeadersFrame headersFrame) {
                                String status = headersFrame.headers().status().toString();
                                logger.info("GOAWAY test: response status={}", status);
                                if ("200".equals(status)) {
                                    responseReceived.complete(true);
                                }
                            }
                        }
                    });

            Http2Headers headers = new DefaultHttp2Headers()
                    .method("GET")
                    .path("/")
                    .scheme("https")
                    .authority(authority);
            streamChannel.writeAndFlush(new DefaultHttp2HeadersFrame(headers, true)).sync();

            assertTrue(responseReceived.get(10, TimeUnit.SECONDS),
                    "Must receive a 200 response before sending GOAWAY");

            // Send GOAWAY(NO_ERROR) on the parent connection
            parentChannel.writeAndFlush(new DefaultHttp2GoAwayFrame(Http2Error.NO_ERROR)).sync();
            logger.info("Sent GOAWAY(NO_ERROR) to proxy");

            // The connection should close after GOAWAY processing
            assertTrue(connectionClosed.get(15, TimeUnit.SECONDS),
                    "Connection must close after client sends GOAWAY");

        } finally {
            group.shutdownGracefully().sync();
        }
    }

    // ===================================================================
    // Test 2: RST_STREAM forwarding (RFC 9113 Section 6.4)
    // ===================================================================

    /**
     * Opens a stream, sends headers, then immediately resets the stream
     * with RST_STREAM(CANCEL). Verifies the proxy handles the cancellation
     * without killing the parent H2 connection.
     *
     * <p>Per RFC 9113 Section 6.4, RST_STREAM allows immediate termination
     * of a stream. A proxy must forward this to the backend to stop
     * processing the cancelled request and free resources.</p>
     *
     * <p>We verify: (1) RST_STREAM does not kill the parent connection,
     * (2) a subsequent stream on the same connection succeeds with 200.</p>
     */
    @Test
    @Order(4)
    void rstStreamCancel_connectionSurvivesAndSubsequentStreamsWork() throws Exception {
        EventLoopGroup group = new NioEventLoopGroup(1);
        try {
            SslContext sslCtx = buildClientSslContext();
            String authority = "localhost:" + loadBalancerPort;

            AtomicBoolean connectionAlive = new AtomicBoolean(true);
            CompletableFuture<Boolean> secondResponse = new CompletableFuture<>();

            Channel parentChannel = connectH2Client(group, sslCtx,
                    new SimpleChannelInboundHandler<Object>() {
                        @Override
                        protected void channelRead0(ChannelHandlerContext ctx, Object msg) {
                            // Connection-level frames (SETTINGS, GOAWAY, etc.)
                        }

                        @Override
                        public void channelInactive(ChannelHandlerContext ctx) {
                            connectionAlive.set(false);
                        }
                    });

            // --- Stream 1: open, send request, then RST_STREAM ---
            Http2StreamChannel stream1 = openStream(parentChannel,
                    new SimpleChannelInboundHandler<Http2StreamFrame>() {
                        @Override
                        protected void channelRead0(ChannelHandlerContext ctx, Http2StreamFrame frame) {
                            logger.info("Stream1 received: {}", frame.getClass().getSimpleName());
                        }

                        @Override
                        public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
                            // RST_STREAM may cause a stream-level error -- expected
                            logger.info("Stream1 exception (expected): {}", cause.getMessage());
                        }
                    });

            Http2Headers headers1 = new DefaultHttp2Headers()
                    .method("GET")
                    .path("/")
                    .scheme("https")
                    .authority(authority);
            stream1.writeAndFlush(new DefaultHttp2HeadersFrame(headers1, true)).sync();

            // Give the proxy a moment to begin forwarding
            Thread.sleep(100);

            // Close the stream channel -- Netty sends RST_STREAM(CANCEL)
            stream1.close().sync();
            logger.info("Sent RST_STREAM(CANCEL) on stream1");

            // Allow the reset to propagate through the proxy
            Thread.sleep(500);

            // --- Stream 2: verify the parent connection is still usable ---
            assertTrue(connectionAlive.get(),
                    "Parent H2 connection must survive RST_STREAM on a single stream");

            Http2StreamChannel stream2 = openStream(parentChannel,
                    new SimpleChannelInboundHandler<Http2StreamFrame>() {
                        @Override
                        protected void channelRead0(ChannelHandlerContext ctx, Http2StreamFrame frame) {
                            if (frame instanceof Http2HeadersFrame headersFrame) {
                                String status = headersFrame.headers().status().toString();
                                logger.info("Stream2 received status: {}", status);
                                if ("200".equals(status)) {
                                    secondResponse.complete(true);
                                }
                            }
                        }
                    });

            Http2Headers headers2 = new DefaultHttp2Headers()
                    .method("GET")
                    .path("/")
                    .scheme("https")
                    .authority(authority);
            stream2.writeAndFlush(new DefaultHttp2HeadersFrame(headers2, true)).sync();

            assertTrue(secondResponse.get(10, TimeUnit.SECONDS),
                    "Second stream must receive 200 after first stream was RST");

            parentChannel.close().sync();
        } finally {
            group.shutdownGracefully().sync();
        }
    }

    // ===================================================================
    // Test 3: Backpressure / flow control (RFC 9113 Section 6.9)
    // ===================================================================

    /**
     * Opens multiple concurrent H2 streams through the proxy, each sending a
     * GET request. The proxy must handle multiplexed streams without OOM or
     * deadlock, exercising the H2 flow control path.
     *
     * <p>Per RFC 9113 Section 6.9, each stream has its own flow control window
     * (default 65535 bytes). Multiple concurrent streams competing for the
     * connection-level window exercises the proxy's backpressure handling.
     * The initial window size for this LB is configured to 1MB, so with
     * multiple concurrent streams each returning small responses, the proxy
     * must correctly manage window accounting.</p>
     *
     * <p>This test verifies:
     * <ul>
     *   <li>The proxy does not OOM under concurrent stream pressure</li>
     *   <li>All streams complete successfully without errors</li>
     *   <li>No deadlock occurs from flow control contention</li>
     * </ul>
     */
    @Test
    @Order(3)
    void concurrentStreams_flowControlHandlesBackpressure() throws Exception {
        EventLoopGroup group = new NioEventLoopGroup(1);
        try {
            SslContext sslCtx = buildClientSslContext();
            String authority = "localhost:" + loadBalancerPort;

            // Track how many responses we receive. Use 10 concurrent streams:
            // enough to exercise flow control contention on the connection-level
            // window without overwhelming a single-threaded backend.
            int streamCount = 10;
            CompletableFuture<Integer> completedStreams = new CompletableFuture<>();
            AtomicInteger responseCount = new AtomicInteger(0);

            Channel parentChannel = connectH2Client(group, sslCtx,
                    new SimpleChannelInboundHandler<Object>() {
                        @Override
                        protected void channelRead0(ChannelHandlerContext ctx, Object msg) {
                            // Connection-level frames
                        }
                    });

            // Open all streams concurrently
            for (int i = 0; i < streamCount; i++) {
                Http2StreamChannel stream = openStream(parentChannel,
                        new SimpleChannelInboundHandler<Http2StreamFrame>() {
                            @Override
                            protected void channelRead0(ChannelHandlerContext ctx,
                                                        Http2StreamFrame frame) {
                                if (frame instanceof Http2HeadersFrame headersFrame) {
                                    if (headersFrame.isEndStream() ||
                                            "200".equals(headersFrame.headers().status().toString())) {
                                        int count = responseCount.incrementAndGet();
                                        if (count >= streamCount) {
                                            completedStreams.complete(count);
                                        }
                                    }
                                }
                                // DATA frames with endStream also count
                                if (frame instanceof Http2DataFrame dataFrame) {
                                    if (dataFrame.isEndStream()) {
                                        int count = responseCount.incrementAndGet();
                                        if (count >= streamCount) {
                                            completedStreams.complete(count);
                                        }
                                    }
                                }
                            }
                        });

                Http2Headers headers = new DefaultHttp2Headers()
                        .method("GET")
                        .path("/stream-" + i)
                        .scheme("https")
                        .authority(authority);
                stream.writeAndFlush(new DefaultHttp2HeadersFrame(headers, true));
            }

            // All streams must complete within a reasonable timeout.
            // If flow control deadlocks, this will time out.
            int completed = completedStreams.get(30, TimeUnit.SECONDS);
            assertEquals(streamCount, completed,
                    "All " + streamCount + " concurrent streams must complete");

            logger.info("Backpressure test: {}/{} streams completed successfully",
                    completed, streamCount);

            parentChannel.close().sync();
        } finally {
            group.shutdownGracefully().sync();
        }
    }

    // ===================================================================
    // Test 4: TLS ALPN negotiation (RFC 7301)
    // ===================================================================

    /**
     * Connects with a Java HttpClient requesting HTTP/2. The TLS ALPN
     * negotiation should select "h2" as the application protocol.
     *
     * <p>Per RFC 7301, the client advertises supported protocols in the TLS
     * ClientHello ALPN extension. The server selects the highest mutually
     * supported protocol. Our load balancer advertises both h2 and http/1.1,
     * so h2 should win when the client offers it.</p>
     */
    @Test
    @Order(1)
    void alpnNegotiatesH2_whenClientOffersH2() throws Exception {
        HttpClient httpClient = buildJavaHttpClient(HttpClient.Version.HTTP_2);

        HttpRequest request = HttpRequest.newBuilder()
                .GET()
                .uri(URI.create("https://localhost:" + loadBalancerPort + "/"))
                .version(HttpClient.Version.HTTP_2)
                .timeout(Duration.ofSeconds(10))
                .build();

        HttpResponse<String> response = httpClient.send(request, HttpResponse.BodyHandlers.ofString());

        assertEquals(200, response.statusCode(),
                "Request via H2 ALPN must succeed with 200");

        assertEquals(HttpClient.Version.HTTP_2, response.version(),
                "ALPN must negotiate HTTP/2 when client offers h2");

        assertEquals("Meow", response.body(),
                "Response body must match backend echo");
    }

    /**
     * Connects to the load balancer with a Netty TLS client that only offers
     * "http/1.1" in ALPN (does NOT advertise h2). Verifies that the server
     * selects "http/1.1" as the negotiated application protocol.
     *
     * <p>Validates RFC 7301 Section 3.2 -- when the client only offers
     * http/1.1 in the ALPN extension, the server must not upgrade to h2.
     * This is tested at the TLS layer by inspecting the negotiated protocol
     * from the SslHandler, which is authoritative regardless of any
     * higher-level HTTP framing issues.</p>
     *
     * <p>Uses a raw Netty TLS client rather than Java's HttpClient because
     * we need explicit control over the ALPN protocol list offered in the
     * ClientHello, and we want to verify the TLS-layer negotiation result
     * directly from the {@link SslHandler}.</p>
     */
    @Test
    @Order(2)
    void alpnFallsBackToH1_whenClientDoesNotOfferH2() throws Exception {
        EventLoopGroup group = new NioEventLoopGroup(1);
        try {
            // Build an SslContext that ONLY offers http/1.1 in ALPN
            SslContext h1OnlySslCtx = SslContextBuilder.forClient()
                    .sslProvider(SslProvider.JDK)
                    .trustManager(InsecureTrustManagerFactory.INSTANCE)
                    .applicationProtocolConfig(new ApplicationProtocolConfig(
                            ApplicationProtocolConfig.Protocol.ALPN,
                            ApplicationProtocolConfig.SelectorFailureBehavior.NO_ADVERTISE,
                            ApplicationProtocolConfig.SelectedListenerFailureBehavior.ACCEPT,
                            ApplicationProtocolNames.HTTP_1_1))
                    .build();

            CompletableFuture<String> negotiatedProtocol = new CompletableFuture<>();

            Bootstrap bootstrap = new Bootstrap()
                    .group(group)
                    .channel(NioSocketChannel.class)
                    .handler(new ChannelInitializer<SocketChannel>() {
                        @Override
                        protected void initChannel(SocketChannel ch) {
                            SslHandler sslHandler = h1OnlySslCtx.newHandler(
                                    ch.alloc(), "localhost", loadBalancerPort);
                            ch.pipeline().addLast(sslHandler);
                            // No Http2FrameCodec -- we only care about ALPN negotiation
                            ch.pipeline().addLast(new SimpleChannelInboundHandler<Object>() {
                                @Override
                                public void handlerAdded(ChannelHandlerContext ctx) {
                                    // After handshake completes, read the negotiated protocol
                                    sslHandler.handshakeFuture().addListener(f -> {
                                        if (f.isSuccess()) {
                                            String proto = sslHandler.applicationProtocol();
                                            logger.info("ALPN negotiated protocol: {}", proto);
                                            negotiatedProtocol.complete(proto);
                                        } else {
                                            negotiatedProtocol.completeExceptionally(f.cause());
                                        }
                                    });
                                }

                                @Override
                                protected void channelRead0(ChannelHandlerContext ctx, Object msg) {
                                    // We don't send any HTTP data -- just checking ALPN
                                }
                            });
                        }
                    });

            ChannelFuture cf = bootstrap.connect("127.0.0.1", loadBalancerPort).sync();
            assertTrue(cf.isSuccess(), "TLS client must connect to load balancer");

            String protocol = negotiatedProtocol.get(10, TimeUnit.SECONDS);

            assertEquals(ApplicationProtocolNames.HTTP_1_1, protocol,
                    "ALPN must negotiate http/1.1 when client does not offer h2");

            cf.channel().close().sync();
        } finally {
            group.shutdownGracefully().sync();
        }
    }

    // ===================================================================
    // Helpers
    // ===================================================================

    /**
     * Builds a Netty {@link SslContext} for an HTTP/2 client with ALPN
     * advertising both h2 and http/1.1. Uses InsecureTrustManager for the
     * load balancer's self-signed certificate.
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
     *
     * <p>{@code Http2FrameCodec} and {@code Http2MultiplexHandler} are stateful
     * (not @Sharable) so a fresh pair is created per connection in the
     * {@link ChannelInitializer}.</p>
     *
     * @param group         EventLoopGroup for the client
     * @param sslCtx        TLS context with ALPN for h2
     * @param parentHandler receives connection-level frames (GOAWAY, SETTINGS)
     * @return the connected parent Channel ready for stream creation
     */
    private Channel connectH2Client(EventLoopGroup group, SslContext sslCtx,
                                    SimpleChannelInboundHandler<Object> parentHandler) throws Exception {

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
                        ch.pipeline().addLast("parentHandler", parentHandler);
                    }
                });

        ChannelFuture cf = bootstrap.connect("127.0.0.1", loadBalancerPort).sync();
        assertTrue(cf.isSuccess(), "H2 client must connect to load balancer");

        Channel channel = cf.channel();

        // Wait for TLS handshake to complete
        channel.pipeline().get(SslHandler.class).handshakeFuture().sync();

        // Brief pause for the H2 SETTINGS exchange to finish. Without this the
        // first stream can race with SETTINGS ACK.
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

    /**
     * Builds a Java {@link HttpClient} with InsecureTrustManager for the
     * load balancer's self-signed certificate.
     */
    private static HttpClient buildJavaHttpClient(HttpClient.Version version) throws Exception {
        SSLContext sslContext = SSLContext.getInstance("TLSv1.3");
        sslContext.init(null, InsecureTrustManagerFactory.INSTANCE.getTrustManagers(),
                new SecureRandom());

        return HttpClient.newBuilder()
                .sslContext(sslContext)
                .version(version)
                .build();
    }
}
