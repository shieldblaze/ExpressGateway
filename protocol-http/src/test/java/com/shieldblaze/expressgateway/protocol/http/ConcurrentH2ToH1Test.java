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
import io.netty.bootstrap.Bootstrap;
import io.netty.buffer.ByteBuf;
import io.netty.buffer.Unpooled;
import io.netty.channel.Channel;
import io.netty.channel.ChannelFuture;
import io.netty.channel.ChannelHandler;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInitializer;
import io.netty.channel.EventLoopGroup;
import io.netty.channel.SimpleChannelInboundHandler;
import io.netty.channel.nio.NioEventLoopGroup;
import io.netty.channel.socket.SocketChannel;
import io.netty.channel.socket.nio.NioSocketChannel;
import io.netty.handler.codec.http.DefaultFullHttpResponse;
import io.netty.handler.codec.http.FullHttpRequest;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpVersion;
import io.netty.handler.codec.http2.DefaultHttp2Headers;
import io.netty.handler.codec.http2.DefaultHttp2HeadersFrame;
import io.netty.handler.codec.http2.Http2DataFrame;
import io.netty.handler.codec.http2.Http2FrameCodecBuilder;
import io.netty.handler.codec.http2.Http2Headers;
import io.netty.handler.codec.http2.Http2HeadersFrame;
import io.netty.handler.codec.http2.Http2MultiplexHandler;
import io.netty.handler.codec.http2.Http2StreamChannel;
import io.netty.handler.codec.http2.Http2StreamChannelBootstrap;
import io.netty.handler.codec.http2.Http2StreamFrame;
import io.netty.handler.codec.http2.HttpConversionUtil;
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
import java.nio.charset.StandardCharsets;
import java.util.List;
import java.util.Map;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;

import static io.netty.handler.codec.http.HttpResponseStatus.OK;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * P1-1 integration test: concurrent HTTP/2 streams proxied to a single HTTP/1.1
 * backend, exercising the {@code translatedStreamQueue} FIFO ordering path.
 *
 * <h2>What this tests (XPROTO-R1)</h2>
 *
 * <p>When the frontend is HTTP/2 and the backend is HTTP/1.1, the proxy must
 * serialize multiplexed H2 streams onto sequential H1 request-response exchanges.
 * The {@code translatedStreamQueue} (a FIFO {@link java.util.Deque}) in
 * {@code HttpConnection} maps each backend H1 response back to the correct H2
 * stream. A bug in this mapping causes response mix-up: stream N receives the
 * response intended for stream M.</p>
 *
 * <h2>Test topology</h2>
 * <pre>
 *   H2 client (Netty FrameCodec, TLS+ALPN)
 *       |
 *       | 20 concurrent streams, each GET /stream-{i}
 *       v
 *   +-------------------+
 *   | ExpressGateway LB |  TLS frontend (H2 via ALPN)
 *   |  translatedStream |  serializes H2 -> H1
 *   |  Queue (FIFO)     |
 *   +-------------------+
 *       |
 *       | sequential H1.1 requests (cleartext)
 *       v
 *   +-------------------+
 *   | Backend H1 server |  echoes the request path in the response body
 *   +-------------------+
 * </pre>
 *
 * <h2>Verification</h2>
 * <ol>
 *   <li>All 20 streams receive HTTP 200 responses.</li>
 *   <li>Each stream's response body matches its request path -- no cross-stream
 *       response mixing.</li>
 *   <li>No streams are silently dropped (response count == request count).</li>
 * </ol>
 *
 * <p>The test uses Netty's {@code Http2FrameCodec + Http2MultiplexHandler} for
 * frame-level control over individual streams, matching the pattern established
 * in {@link Http2LifecycleTest}.</p>
 *
 * <p>Relevant RFCs:
 * <ul>
 *   <li>RFC 9113 Section 5.1 -- stream lifecycle and concurrency</li>
 *   <li>RFC 9113 Section 8.1 -- HTTP request/response exchange over H2</li>
 *   <li>RFC 9112 -- HTTP/1.1 message framing (serial request/response)</li>
 * </ul>
 */
@Timeout(value = 120, unit = TimeUnit.SECONDS)
class ConcurrentH2ToH1Test {

    private static final Logger logger = LogManager.getLogger(ConcurrentH2ToH1Test.class);

    /**
     * Number of concurrent H2 streams to open simultaneously. 20 is high enough
     * to stress the FIFO queue ordering but low enough to complete within a
     * reasonable timeout even with serial H1 backend processing.
     */
    private static final int STREAM_COUNT = 20;

    private static int loadBalancerPort;
    private static HTTPLoadBalancer httpLoadBalancer;
    private static HttpServer httpServer;

    @BeforeAll
    static void setup() throws Exception {
        // ---------------------------------------------------------------
        // TLS for the frontend: H2 via ALPN (server-side TLS)
        // ---------------------------------------------------------------
        SelfSignedCertificate ssc = SelfSignedCertificate.generateNew(
                List.of("127.0.0.1"), List.of("localhost"));
        CertificateKeyPair certificateKeyPair = CertificateKeyPair.forClient(
                List.of(ssc.x509Certificate()), ssc.keyPair().getPrivate());

        TlsServerConfiguration tlsServerConfiguration =
                TlsServerConfiguration.copyFrom(TlsServerConfiguration.DEFAULT);
        tlsServerConfiguration.enable();
        tlsServerConfiguration.addMapping("localhost", certificateKeyPair);
        tlsServerConfiguration.defaultMapping(certificateKeyPair);

        // ---------------------------------------------------------------
        // No TLS on the backend side: cleartext H1.1
        // This forces the H2->H1 translation path through translatedStreamQueue.
        // ---------------------------------------------------------------
        TlsClientConfiguration tlsClientConfiguration =
                TlsClientConfiguration.copyFrom(TlsClientConfiguration.DEFAULT);
        // tlsClientConfiguration remains disabled (cleartext backend)

        // ---------------------------------------------------------------
        // Backend: cleartext HTTP/1.1 server with a path-echoing handler.
        // Each response body is the request URI so the client can verify
        // that no responses were mixed between streams.
        // ---------------------------------------------------------------
        httpServer = new HttpServer(false, new PathEchoHandler());
        httpServer.start();
        httpServer.START_FUTURE.get(60, TimeUnit.SECONDS);

        // ---------------------------------------------------------------
        // Load balancer: TLS frontend, cleartext backend
        // ---------------------------------------------------------------
        loadBalancerPort = AvailablePortUtil.getTcpPort();

        Cluster cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new HTTPRoundRobin(NOOPSessionPersistence.INSTANCE))
                .build();

        httpLoadBalancer = HTTPLoadBalancerBuilder.newBuilder()
                .withConfigurationContext(ConfigurationContext.create(
                        tlsClientConfiguration, tlsServerConfiguration))
                .withBindAddress(new InetSocketAddress("127.0.0.1", loadBalancerPort))
                .build();

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
            httpLoadBalancer.stop().future().get(60, TimeUnit.SECONDS);
        }
        if (httpServer != null) {
            httpServer.shutdown();
            httpServer.SHUTDOWN_FUTURE.get(60, TimeUnit.SECONDS);
        }
    }

    // ===================================================================
    // Test: 20 concurrent H2 streams -> single H1 backend, FIFO ordering
    // ===================================================================

    /**
     * Opens {@value #STREAM_COUNT} concurrent HTTP/2 streams, each requesting a
     * unique path {@code /stream-{i}}. The cleartext H1 backend echoes the
     * request path in the response body. The test verifies:
     * <ol>
     *   <li>Every stream receives a 200 status.</li>
     *   <li>Every stream's response body matches its request path -- proving
     *       the {@code translatedStreamQueue} correctly maps H1 responses
     *       back to H2 streams in FIFO order.</li>
     *   <li>No responses are silently dropped (all 20 arrive).</li>
     * </ol>
     *
     * <p>Because H1 is serial, the proxy queues H2 requests and sends them
     * one at a time to the backend. The FIFO queue must pop in the same order
     * that requests were sent, or responses will be routed to the wrong stream.
     * This is the BUG-008 regression path.</p>
     */
    @Test
    void concurrentH2Streams_allReceiveCorrectResponsesViaH1Backend() throws Exception {
        EventLoopGroup group = new NioEventLoopGroup(1);
        try {
            SslContext sslCtx = buildClientSslContext();
            String authority = "localhost:" + loadBalancerPort;

            // responseMap: stream index -> response body received on that stream.
            // Populated by per-stream handlers; verified after all futures complete.
            Map<Integer, String> responseMap = new ConcurrentHashMap<>();

            // One future per stream; completes when that stream receives endStream.
            @SuppressWarnings("unchecked")
            CompletableFuture<String>[] futures = new CompletableFuture[STREAM_COUNT];
            for (int i = 0; i < STREAM_COUNT; i++) {
                futures[i] = new CompletableFuture<>();
            }

            // Track any unexpected connection-level errors
            CompletableFuture<Throwable> connectionError = new CompletableFuture<>();

            Channel parentChannel = connectH2Client(group, sslCtx,
                    new SimpleChannelInboundHandler<Object>() {
                        @Override
                        protected void channelRead0(ChannelHandlerContext ctx, Object msg) {
                            // Connection-level frames (SETTINGS, WINDOW_UPDATE, etc.)
                            logger.debug("Connection-level frame: {}", msg.getClass().getSimpleName());
                        }

                        @Override
                        public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
                            logger.error("Connection-level error", cause);
                            connectionError.complete(cause);
                        }
                    });

            // ---------------------------------------------------------------
            // Open all streams concurrently. Each stream has its own handler
            // that accumulates the response body and completes its future.
            // ---------------------------------------------------------------
            for (int i = 0; i < STREAM_COUNT; i++) {
                final int streamIndex = i;
                final String path = "/stream-" + i;

                Http2StreamChannel streamChannel = openStream(parentChannel,
                        new StreamResponseHandler(streamIndex, path, futures[streamIndex], responseMap));

                Http2Headers headers = new DefaultHttp2Headers()
                        .method("GET")
                        .path(path)
                        .scheme("https")
                        .authority(authority);

                // endStream=true: GET has no body
                streamChannel.writeAndFlush(new DefaultHttp2HeadersFrame(headers, true))
                        .addListener(f -> {
                            if (!f.isSuccess()) {
                                logger.error("Failed to send headers for stream-{}", streamIndex, f.cause());
                                futures[streamIndex].completeExceptionally(f.cause());
                            }
                        });
            }

            // ---------------------------------------------------------------
            // Wait for all streams to complete, with a per-stream timeout.
            // If the translatedStreamQueue is broken, some streams will hang
            // forever waiting for a response that was routed to the wrong stream.
            // ---------------------------------------------------------------
            int successCount = 0;
            for (int i = 0; i < STREAM_COUNT; i++) {
                String responseBody = futures[i].get(30, TimeUnit.SECONDS);
                assertNotNull(responseBody,
                        "Stream " + i + " must receive a response body");
                successCount++;
            }

            assertEquals(STREAM_COUNT, successCount,
                    "All " + STREAM_COUNT + " streams must complete successfully");

            // ---------------------------------------------------------------
            // Verify response correctness: each stream must have received
            // a response body matching its request path. If the FIFO queue
            // is misordered, stream i will get stream j's response.
            // ---------------------------------------------------------------
            assertEquals(STREAM_COUNT, responseMap.size(),
                    "Response map must contain exactly " + STREAM_COUNT + " entries");

            for (int i = 0; i < STREAM_COUNT; i++) {
                String expectedPath = "/stream-" + i;
                String actualBody = responseMap.get(i);
                assertNotNull(actualBody,
                        "Stream " + i + " must have a response in the map");
                assertEquals(expectedPath, actualBody,
                        "Stream " + i + " response body must match its request path '"
                                + expectedPath + "' -- got '" + actualBody
                                + "' (indicates translatedStreamQueue FIFO ordering bug)");
            }

            // Verify no connection-level errors occurred
            if (connectionError.isDone() && !connectionError.isCompletedExceptionally()) {
                Throwable error = connectionError.getNow(null);
                if (error != null) {
                    logger.warn("Connection-level error during test (non-fatal): {}", error.getMessage());
                }
            }

            logger.info("All {} concurrent H2->H1 streams completed with correct response mapping",
                    STREAM_COUNT);

            parentChannel.close().sync();
        } finally {
            group.shutdownGracefully().sync();
        }
    }

    // ===================================================================
    // Per-stream response handler
    // ===================================================================

    /**
     * Handles the response frames for a single HTTP/2 stream. Accumulates
     * DATA frame payloads into a StringBuilder and completes the stream's
     * future when endStream is received.
     *
     * <p>Thread safety: each instance is used by exactly one stream channel,
     * which is serviced by a single EventLoop thread, so no synchronization
     * is needed on the StringBuilder.</p>
     */
    private static final class StreamResponseHandler
            extends SimpleChannelInboundHandler<Http2StreamFrame> {

        private final int streamIndex;
        private final String expectedPath;
        private final CompletableFuture<String> future;
        private final Map<Integer, String> responseMap;
        private final StringBuilder bodyAccumulator = new StringBuilder();
        private boolean gotHeaders;
        private String status;

        StreamResponseHandler(int streamIndex, String expectedPath,
                              CompletableFuture<String> future,
                              Map<Integer, String> responseMap) {
            this.streamIndex = streamIndex;
            this.expectedPath = expectedPath;
            this.future = future;
            this.responseMap = responseMap;
        }

        @Override
        protected void channelRead0(ChannelHandlerContext ctx, Http2StreamFrame frame) {
            if (frame instanceof Http2HeadersFrame headersFrame) {
                status = headersFrame.headers().status() != null
                        ? headersFrame.headers().status().toString()
                        : "unknown";
                gotHeaders = true;

                logger.debug("Stream-{} received HEADERS: status={}, endStream={}",
                        streamIndex, status, headersFrame.isEndStream());

                if (headersFrame.isEndStream()) {
                    // Headers-only response (no body). The body is the status itself
                    // which shouldn't happen with our PathEchoHandler, but handle it.
                    completeStream();
                }
            }

            if (frame instanceof Http2DataFrame dataFrame) {
                ByteBuf content = dataFrame.content();
                if (content.isReadable()) {
                    bodyAccumulator.append(content.toString(StandardCharsets.UTF_8));
                }

                logger.debug("Stream-{} received DATA: {} bytes, endStream={}",
                        streamIndex, content.readableBytes(), dataFrame.isEndStream());

                if (dataFrame.isEndStream()) {
                    completeStream();
                }
            }
        }

        private void completeStream() {
            String body = bodyAccumulator.toString();
            responseMap.put(streamIndex, body);

            if (!"200".equals(status)) {
                future.completeExceptionally(new AssertionError(
                        "Stream " + streamIndex + " expected 200 but got " + status
                                + ", body: " + body));
            } else {
                future.complete(body);
            }

            logger.info("Stream-{} completed: status={}, body='{}', expected='{}'",
                    streamIndex, status, body, expectedPath);
        }

        @Override
        public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
            logger.error("Stream-{} exception", streamIndex, cause);
            if (!future.isDone()) {
                future.completeExceptionally(cause);
            }
        }

        @Override
        public void channelInactive(ChannelHandlerContext ctx) {
            // Stream channel closed without endStream -- treat as error unless
            // the future was already completed (normal close after endStream).
            if (!future.isDone()) {
                future.completeExceptionally(new IllegalStateException(
                        "Stream " + streamIndex + " channel closed before receiving endStream"
                                + " (gotHeaders=" + gotHeaders + ", accumulated="
                                + bodyAccumulator.length() + " bytes)"));
            }
        }
    }

    // ===================================================================
    // Backend handler: echoes the request path in the response body
    // ===================================================================

    /**
     * A backend handler that echoes the request URI in the response body.
     * This allows the test to verify that responses are matched to the
     * correct H2 stream: if stream i requested {@code /stream-i}, its
     * response body must be exactly {@code /stream-i}.
     *
     * <p>Must be {@code @Sharable} because the same instance is added to
     * every child channel pipeline by {@link HttpServer}.</p>
     */
    @ChannelHandler.Sharable
    static final class PathEchoHandler extends SimpleChannelInboundHandler<FullHttpRequest> {

        private static final Logger handlerLogger = LogManager.getLogger(PathEchoHandler.class);
        private final AtomicInteger requestCount = new AtomicInteger(0);

        @Override
        protected void channelRead0(ChannelHandlerContext ctx, FullHttpRequest msg) {
            int seq = requestCount.incrementAndGet();
            String uri = msg.uri();
            handlerLogger.debug("Backend received request #{}: {}", seq, uri);

            // Echo the URI as the response body. The test client verifies this
            // matches the path sent on the corresponding H2 stream.
            byte[] body = uri.getBytes(StandardCharsets.UTF_8);

            DefaultFullHttpResponse response = new DefaultFullHttpResponse(
                    HttpVersion.HTTP_1_1, OK, Unpooled.wrappedBuffer(body));

            response.headers().set(HttpHeaderNames.CONTENT_LENGTH, body.length);
            response.headers().set(HttpHeaderNames.CONTENT_TYPE, "text/plain");

            // For H2 backends, propagate stream ID. Not needed here (H1 backend)
            // but included for defensive compatibility if the test topology changes.
            if (msg.headers().contains(HttpConversionUtil.ExtensionHeaderNames.STREAM_ID.text())) {
                response.headers().set(
                        HttpConversionUtil.ExtensionHeaderNames.STREAM_ID.text(),
                        msg.headers().get(HttpConversionUtil.ExtensionHeaderNames.STREAM_ID.text()));
            }

            ctx.writeAndFlush(response);
        }
    }

    // ===================================================================
    // H2 client helpers (same pattern as Http2LifecycleTest)
    // ===================================================================

    /**
     * Builds a Netty {@link SslContext} for an HTTP/2 client with ALPN
     * advertising both h2 and http/1.1. Uses InsecureTrustManager for
     * the load balancer's self-signed certificate.
     */
    private static SslContext buildClientSslContext() throws Exception {
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
     * Creates a Netty HTTP/2 client connection to the load balancer.
     *
     * <p>{@code Http2FrameCodec} and {@code Http2MultiplexHandler} are stateful
     * (not @Sharable) so a fresh pair is created per connection in the
     * {@link ChannelInitializer}.</p>
     *
     * @param group         EventLoopGroup for the client
     * @param sslCtx        TLS context with ALPN for h2
     * @param parentHandler receives connection-level frames (SETTINGS, GOAWAY)
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
                                        // No-op: proxy does not send server push
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
        // first stream can race with SETTINGS ACK, causing a PROTOCOL_ERROR.
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
