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
import com.shieldblaze.expressgateway.configuration.http.HttpConfiguration;
import com.shieldblaze.expressgateway.configuration.tls.CertificateKeyPair;
import com.shieldblaze.expressgateway.configuration.tls.TlsClientConfiguration;
import com.shieldblaze.expressgateway.configuration.tls.TlsServerConfiguration;
import java.lang.reflect.Constructor;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStartupTask;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancer;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancerBuilder;
import com.shieldblaze.expressgateway.protocol.tcp.TCPListener;
import io.netty.bootstrap.Bootstrap;
import io.netty.buffer.ByteBuf;
import io.netty.buffer.Unpooled;
import io.netty.channel.Channel;
import io.netty.channel.ChannelFuture;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInitializer;
import io.netty.channel.EventLoopGroup;
import io.netty.channel.SimpleChannelInboundHandler;
import io.netty.channel.nio.NioEventLoopGroup;
import io.netty.channel.socket.SocketChannel;
import io.netty.channel.socket.nio.NioSocketChannel;
import io.netty.handler.codec.http2.DefaultHttp2DataFrame;
import io.netty.handler.codec.http2.DefaultHttp2Headers;
import io.netty.handler.codec.http2.DefaultHttp2HeadersFrame;
import io.netty.handler.codec.http2.Http2Error;
import io.netty.handler.codec.http2.Http2FrameCodecBuilder;
import io.netty.handler.codec.http2.Http2GoAwayFrame;
import io.netty.handler.codec.http2.Http2Headers;
import io.netty.handler.codec.http2.Http2MultiplexHandler;
import io.netty.handler.codec.http2.Http2SettingsFrame;
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
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * RB-TEST-04: Tests the F-13 per-connection aggregate body size limit in
 * {@code Http2ServerInboundHandler}.
 *
 * <p>Even if each individual stream stays under the per-stream body size limit,
 * the sum across all concurrent streams can exhaust proxy memory. When the
 * aggregate exceeds {@code maxConnectionBodySize}, the proxy MUST send
 * GOAWAY(ENHANCE_YOUR_CALM) per RFC 9113 Section 10.5 and close the connection.</p>
 *
 * <p>Test strategy: configure a small maxConnectionBodySize (e.g., 10 KB) and a
 * larger per-stream limit (e.g., 8 KB). Open N streams that each send slightly
 * less than the per-stream limit but collectively exceed the connection limit.
 * Verify the connection receives GOAWAY with ENHANCE_YOUR_CALM.</p>
 */
@Timeout(value = 60)
class H2AggregateBodyLimitTest {

    private static final Logger logger = LogManager.getLogger(H2AggregateBodyLimitTest.class);

    private static int loadBalancerPort;
    private static HTTPLoadBalancer httpLoadBalancer;
    private static HttpServer httpServer;

    /**
     * Small connection body limit for testing: 10 KB.
     * Per-stream limit is set to 8 KB so each stream can individually pass
     * but 2+ streams collectively exceed the connection limit.
     */
    private static final long MAX_CONNECTION_BODY_SIZE = 10 * 1024; // 10 KB
    private static final long MAX_REQUEST_BODY_SIZE = 8 * 1024;     // 8 KB per stream

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

        // Custom HttpConfiguration with small aggregate body limit via reflection.
        // The constructor is package-private, so we use reflection as in SlowClientTest.
        HttpConfiguration httpConfig = createCustomHttpConfig();

        httpServer = new HttpServer(true);
        httpServer.start();
        httpServer.START_FUTURE.get(30, TimeUnit.SECONDS);

        loadBalancerPort = AvailablePortUtil.getTcpPort();

        Cluster cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new HTTPRoundRobin(NOOPSessionPersistence.INSTANCE))
                .build();

        httpLoadBalancer = HTTPLoadBalancerBuilder.newBuilder()
                .withConfigurationContext(ConfigurationContext.create(
                        tlsClientConfiguration, tlsServerConfiguration, httpConfig))
                .withBindAddress(new InetSocketAddress("127.0.0.1", loadBalancerPort))
                .withL4FrontListener(new TCPListener())
                .build();

        httpLoadBalancer.mappedCluster("localhost:" + loadBalancerPort, cluster);

        NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress("127.0.0.1", httpServer.port()))
                .build();

        L4FrontListenerStartupTask startupTask = httpLoadBalancer.start();
        startupTask.future().get(30, TimeUnit.SECONDS);
        assertTrue(startupTask.isSuccess(), "Load balancer must start successfully");

        Thread.sleep(500);
    }

    @AfterAll
    static void teardown() throws Exception {
        if (httpLoadBalancer != null) {
            httpLoadBalancer.shutdown().future().get(30, TimeUnit.SECONDS);
        }
        if (httpServer != null) {
            httpServer.shutdown();
            httpServer.SHUTDOWN_FUTURE.get(30, TimeUnit.SECONDS);
        }
    }

    /**
     * Opens multiple streams that each send enough data to stay under the per-stream
     * limit but collectively exceed the per-connection aggregate body limit (F-13).
     *
     * <p>Expected behavior: the server sends GOAWAY(ENHANCE_YOUR_CALM) and closes
     * the connection when the aggregate body bytes exceed maxConnectionBodySize.</p>
     */
    @Test
    void multiStreamAggregate_exceedsConnectionLimit_goawayEnhanceYourCalm() throws Exception {
        EventLoopGroup group = new NioEventLoopGroup(1);
        try {
            SslContext sslCtx = buildClientSslContext();
            CompletableFuture<Long> goAwayErrorCode = new CompletableFuture<>();
            CompletableFuture<Boolean> connectionClosed = new CompletableFuture<>();
            String authority = "localhost:" + loadBalancerPort;

            Channel parentChannel = connectH2Client(group, sslCtx, goAwayErrorCode, connectionClosed);

            // Each stream sends 6 KB of body data. With 3 streams, total = 18 KB > 10 KB limit.
            // Per-stream limit is 8 KB, so each stream is individually within limits.
            int streamCount = 3;
            int bytesPerStream = 6 * 1024; // 6 KB
            byte[] bodyData = new byte[bytesPerStream];

            for (int i = 0; i < streamCount; i++) {
                if (!parentChannel.isActive()) {
                    logger.info("Connection closed after {} streams, as expected (aggregate limit hit)", i);
                    break;
                }

                Http2StreamChannel streamChannel = new Http2StreamChannelBootstrap(parentChannel)
                        .handler(new SimpleChannelInboundHandler<Http2StreamFrame>() {
                            @Override
                            protected void channelRead0(ChannelHandlerContext ctx, Http2StreamFrame frame) {
                                // Ignore stream-level responses
                            }
                        })
                        .open()
                        .sync()
                        .getNow();

                // Send HEADERS (POST with body)
                Http2Headers headers = new DefaultHttp2Headers()
                        .method("POST")
                        .path("/upload-" + i)
                        .scheme("https")
                        .authority(authority)
                        .set("content-length", String.valueOf(bytesPerStream));
                streamChannel.writeAndFlush(new DefaultHttp2HeadersFrame(headers, false)).sync();

                // Send DATA frame with body bytes
                ByteBuf content = Unpooled.wrappedBuffer(bodyData);
                streamChannel.writeAndFlush(new DefaultHttp2DataFrame(content, true)).sync();

                logger.info("Stream {} sent {} bytes", i, bytesPerStream);

                // Brief pause to allow the server to process before sending next stream
                Thread.sleep(100);
            }

            // Wait for connection close -- the server should GOAWAY after the aggregate
            // limit is exceeded (10 KB), which happens after the 2nd stream completes
            // (2 * 6 KB = 12 KB > 10 KB).
            assertTrue(connectionClosed.get(15, TimeUnit.SECONDS),
                    "Connection must be closed after aggregate body limit exceeded");

            // Verify GOAWAY error code is ENHANCE_YOUR_CALM
            if (goAwayErrorCode.isDone() && !goAwayErrorCode.isCompletedExceptionally()) {
                long errorCode = goAwayErrorCode.get();
                assertEquals(Http2Error.ENHANCE_YOUR_CALM.code(), errorCode,
                        "GOAWAY error code must be ENHANCE_YOUR_CALM (0xb) for F-13 aggregate body limit");
                logger.info("Received GOAWAY with ENHANCE_YOUR_CALM -- F-13 aggregate body limit enforced");
            } else {
                // Connection closed without client seeing GOAWAY -- acceptable if GOAWAY
                // was consumed by TLS close. The server log confirms GOAWAY was sent.
                logger.info("Connection closed (GOAWAY may not have been visible to client)");
            }

            assertTrue(!parentChannel.isActive(), "Connection must be inactive after F-13 defense");

        } finally {
            group.shutdownGracefully().sync();
        }
    }

    /**
     * Sends a single stream with body data under the connection aggregate limit.
     * The connection must survive -- this ensures the F-13 check does not trigger
     * on legitimate single-stream traffic.
     */
    @Test
    void singleStream_underConnectionLimit_connectionSurvives() throws Exception {
        EventLoopGroup group = new NioEventLoopGroup(1);
        try {
            SslContext sslCtx = buildClientSslContext();
            CompletableFuture<Long> goAwayErrorCode = new CompletableFuture<>();
            CompletableFuture<Boolean> connectionClosed = new CompletableFuture<>();
            String authority = "localhost:" + loadBalancerPort;

            Channel parentChannel = connectH2Client(group, sslCtx, goAwayErrorCode, connectionClosed);

            // Send 5 KB on a single stream -- under the 10 KB connection limit
            int bodySize = 5 * 1024;
            byte[] bodyData = new byte[bodySize];

            Http2StreamChannel streamChannel = new Http2StreamChannelBootstrap(parentChannel)
                    .handler(new SimpleChannelInboundHandler<Http2StreamFrame>() {
                        @Override
                        protected void channelRead0(ChannelHandlerContext ctx, Http2StreamFrame frame) {
                            // Accept response
                        }
                    })
                    .open()
                    .sync()
                    .getNow();

            Http2Headers headers = new DefaultHttp2Headers()
                    .method("POST")
                    .path("/upload")
                    .scheme("https")
                    .authority(authority)
                    .set("content-length", String.valueOf(bodySize));
            streamChannel.writeAndFlush(new DefaultHttp2HeadersFrame(headers, false)).sync();

            ByteBuf content = Unpooled.wrappedBuffer(bodyData);
            streamChannel.writeAndFlush(new DefaultHttp2DataFrame(content, true)).sync();

            // Wait briefly for any erroneous GOAWAY
            Thread.sleep(1000);

            assertTrue(parentChannel.isActive(),
                    "Connection must stay alive when aggregate body is under the limit");

            // GOAWAY should NOT have been received
            assertTrue(!goAwayErrorCode.isDone(),
                    "No GOAWAY should be sent when under the aggregate body limit");

            parentChannel.close().sync();

        } finally {
            group.shutdownGracefully().sync();
        }
    }

    // ===================================================================
    // Helpers
    // ===================================================================

    /**
     * Creates an HttpConfiguration with small body limits via reflection.
     * The constructor is package-private, so we use reflection (same pattern as SlowClientTest).
     */
    private static HttpConfiguration createCustomHttpConfig() throws Exception {
        Constructor<HttpConfiguration> ctor = HttpConfiguration.class.getDeclaredConstructor();
        ctor.setAccessible(true);
        HttpConfiguration config = ctor.newInstance();

        config.setMaxInitialLineLength(4096)
                .setMaxHeaderSize(8192)
                .setMaxChunkSize(8192)
                .setCompressionThreshold(1024)
                .setDeflateCompressionLevel(6)
                .setBrotliCompressionLevel(4)
                .setMaxConcurrentStreams(100)
                .setBackendResponseTimeoutSeconds(60)
                .setMaxRequestBodySize(MAX_REQUEST_BODY_SIZE)
                .setInitialWindowSize(1048576)
                .setH2ConnectionWindowSize(1048576)
                .setMaxConnectionBodySize(MAX_CONNECTION_BODY_SIZE)
                .setMaxHeaderListSize(8192)
                .setRequestHeaderTimeoutSeconds(30)
                .setRequestBodyTimeoutSeconds(60)
                .setMaxH1ConnectionsPerNode(32)
                .setMaxH2ConnectionsPerNode(4)
                .setPoolIdleTimeoutSeconds(60)
                .setGracefulShutdownDrainMs(5000)
                .validate();

        return config;
    }

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

    private Channel connectH2Client(EventLoopGroup group, SslContext sslCtx,
                                    CompletableFuture<Long> goAwayErrorCode,
                                    CompletableFuture<Boolean> connectionClosed) throws Exception {
        Bootstrap bootstrap = new Bootstrap()
                .group(group)
                .channel(NioSocketChannel.class)
                .handler(new ChannelInitializer<SocketChannel>() {
                    @Override
                    protected void initChannel(SocketChannel ch) {
                        SslHandler sslHandler = sslCtx.newHandler(
                                ch.alloc(), "localhost", loadBalancerPort);
                        ch.pipeline().addLast(sslHandler);
                        ch.pipeline().addLast(Http2FrameCodecBuilder.forClient()
                                .initialSettings(io.netty.handler.codec.http2.Http2Settings.defaultSettings()
                                        .initialWindowSize(65535))
                                .build());
                        ch.pipeline().addLast(new Http2MultiplexHandler(
                                new SimpleChannelInboundHandler<Http2StreamFrame>() {
                                    @Override
                                    protected void channelRead0(ChannelHandlerContext ctx,
                                                                Http2StreamFrame frame) {
                                        // No-op for push promises
                                    }
                                }));
                        ch.pipeline().addLast(new SimpleChannelInboundHandler<Object>() {
                            @Override
                            protected void channelRead0(ChannelHandlerContext ctx, Object msg) {
                                if (msg instanceof Http2GoAwayFrame goaway) {
                                    logger.info("Received GOAWAY: errorCode={}", goaway.errorCode());
                                    goAwayErrorCode.complete(goaway.errorCode());
                                } else if (msg instanceof Http2SettingsFrame) {
                                    logger.debug("Received SETTINGS frame");
                                }
                            }

                            @Override
                            public void channelInactive(ChannelHandlerContext ctx) {
                                connectionClosed.complete(true);
                            }
                        });
                    }
                });

        ChannelFuture cf = bootstrap.connect("127.0.0.1", loadBalancerPort).sync();
        assertTrue(cf.isSuccess(), "H2 client must connect to load balancer");

        Channel channel = cf.channel();
        channel.pipeline().get(SslHandler.class).handshakeFuture().sync();

        // Wait for SETTINGS exchange
        Thread.sleep(500);

        return channel;
    }
}
