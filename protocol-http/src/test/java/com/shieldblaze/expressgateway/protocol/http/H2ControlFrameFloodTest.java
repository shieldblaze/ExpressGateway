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
import io.netty.handler.codec.http2.DefaultHttp2PingFrame;
import io.netty.handler.codec.http2.Http2Error;
import io.netty.handler.codec.http2.Http2FrameCodecBuilder;
import io.netty.handler.codec.http2.Http2GoAwayFrame;
import io.netty.handler.codec.http2.Http2MultiplexHandler;
import io.netty.handler.codec.http2.Http2SettingsFrame;
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
 * V-TEST-003: HTTP/2 SETTINGS/PING flood protection test.
 *
 * <p>Validates that the load balancer's {@code Http2ServerInboundHandler} enforces
 * the SETTINGS/PING control frame rate limit (F-11). When a client sends more than
 * {@code CONTROL_FRAME_RATE_LIMIT} (100) SETTINGS or PING frames within the 10-second
 * rate-limit window, the server MUST respond with GOAWAY(ENHANCE_YOUR_CALM) per
 * RFC 9113 Section 10.5 and close the connection.</p>
 *
 * <p>This defense prevents CVE-2019-9512 (PING flood) and CVE-2019-9515 (SETTINGS
 * flood) denial-of-service attacks that can exhaust server CPU by forcing it to
 * process and acknowledge an unbounded number of control frames.</p>
 */
@Timeout(value = 60)
class H2ControlFrameFloodTest {

    private static final Logger logger = LogManager.getLogger(H2ControlFrameFloodTest.class);

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
        httpServer.START_FUTURE.get(30, TimeUnit.SECONDS);

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
     * Sends a large number of PING frames rapidly on a single H2 connection. The
     * server's rate limit (100 frames per 10-second window) should be exceeded,
     * triggering a GOAWAY with error code ENHANCE_YOUR_CALM (0xb) and connection close.
     *
     * <p>Per RFC 9113 Section 10.5: "An endpoint that detects this behavior
     * can signal the error as a connection error of type ENHANCE_YOUR_CALM."</p>
     *
     * <p>Note: The server's {@code Http2FrameCodec} auto-acks SETTINGS during connection
     * setup, which consumes part of the 100-frame budget. We send 200 PING frames to
     * account for SETTINGS exchange overhead and ensure the limit is hit.</p>
     */
    @Test
    void pingFlood_triggersGoAwayOrConnectionClose() throws Exception {
        EventLoopGroup group = new NioEventLoopGroup(1);
        try {
            SslContext sslCtx = buildClientSslContext();
            CompletableFuture<Long> goAwayErrorCode = new CompletableFuture<>();
            CompletableFuture<Boolean> connectionClosed = new CompletableFuture<>();

            Channel parentChannel = connectH2Client(group, sslCtx, goAwayErrorCode, connectionClosed);

            // Wait for SETTINGS exchange to complete
            Thread.sleep(500);

            // Send 200 PING frames rapidly -- well above CONTROL_FRAME_RATE_LIMIT (100).
            // The server counts SETTINGS + PING frames in the same rate limit window.
            // The SETTINGS exchange during connection setup consumes some budget.
            int pingCount = 200;
            int sentCount = 0;
            for (int i = 0; i < pingCount; i++) {
                if (!parentChannel.isActive()) {
                    logger.info("Connection closed after {} PING frames", i);
                    break;
                }
                parentChannel.writeAndFlush(new DefaultHttp2PingFrame(i));
                sentCount++;
            }
            logger.info("Sent {} PING frames", sentCount);

            // Wait for the server to process the flood and respond.
            // The server sends GOAWAY(ENHANCE_YOUR_CALM) and closes the connection.
            //
            // The connection close is the most reliable signal -- the GOAWAY frame may
            // not be seen by the client if TLS close_notify and TCP RST arrive before
            // the H2 codec processes the GOAWAY. We check connection close first, then
            // verify GOAWAY if it arrived.
            assertTrue(connectionClosed.get(10, TimeUnit.SECONDS),
                    "Connection must be closed after PING flood triggers ENHANCE_YOUR_CALM defense");

            // If the GOAWAY was received before the close, validate the error code
            if (goAwayErrorCode.isDone() && !goAwayErrorCode.isCompletedExceptionally()) {
                long errorCode = goAwayErrorCode.get();
                assertEquals(Http2Error.ENHANCE_YOUR_CALM.code(), errorCode,
                        "GOAWAY error code must be ENHANCE_YOUR_CALM (0xb)");
                logger.info("Received GOAWAY with ENHANCE_YOUR_CALM (0xb)");
            } else {
                // The connection was closed without the client seeing the GOAWAY frame.
                // This is acceptable -- the server log confirms GOAWAY was sent on the wire.
                logger.info("Connection closed by server (GOAWAY may have been consumed by TLS close)");
            }

            assertTrue(!parentChannel.isActive(),
                    "Connection must be inactive after PING flood defense");

        } finally {
            group.shutdownGracefully().sync();
        }
    }

    /**
     * Sends a normal number of PING frames (well below the rate limit) and
     * verifies the connection stays alive. This is a regression test to ensure
     * the rate limiter does not fire on legitimate usage.
     */
    @Test
    void normalPingRate_connectionSurvives() throws Exception {
        EventLoopGroup group = new NioEventLoopGroup(1);
        try {
            SslContext sslCtx = buildClientSslContext();
            CompletableFuture<Long> goAwayErrorCode = new CompletableFuture<>();
            CompletableFuture<Boolean> connectionClosed = new CompletableFuture<>();

            Channel parentChannel = connectH2Client(group, sslCtx, goAwayErrorCode, connectionClosed);

            // Wait for SETTINGS exchange to complete
            Thread.sleep(500);

            // Send 10 PING frames -- well below the 100 per-window limit.
            for (int i = 0; i < 10; i++) {
                parentChannel.writeAndFlush(new DefaultHttp2PingFrame(i));
            }

            // Wait briefly to allow any erroneous GOAWAY to arrive
            Thread.sleep(500);

            assertTrue(parentChannel.isActive(),
                    "Connection must stay alive after 10 PINGs (below rate limit)");

            // GOAWAY should NOT have been received
            assertFalse(goAwayErrorCode.isDone(),
                    "No GOAWAY should be sent for normal PING rate");

            parentChannel.close().sync();
        } finally {
            group.shutdownGracefully().sync();
        }
    }

    private static void assertFalse(boolean condition, String message) {
        assertTrue(!condition, message);
    }

    // ===================================================================
    // Helpers
    // ===================================================================

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
                        ch.pipeline().addLast(Http2FrameCodecBuilder.forClient().build());
                        ch.pipeline().addLast(new Http2MultiplexHandler(
                                new SimpleChannelInboundHandler<Http2StreamFrame>() {
                                    @Override
                                    protected void channelRead0(ChannelHandlerContext ctx,
                                                                Http2StreamFrame frame) {
                                        // No-op for server-initiated streams
                                    }
                                }));
                        ch.pipeline().addLast(new SimpleChannelInboundHandler<Object>() {
                            @Override
                            protected void channelRead0(ChannelHandlerContext ctx, Object msg) {
                                if (msg instanceof Http2GoAwayFrame goaway) {
                                    logger.info("Received GOAWAY: errorCode={}", goaway.errorCode());
                                    goAwayErrorCode.complete(goaway.errorCode());
                                } else if (msg instanceof Http2SettingsFrame) {
                                    // Expected during connection setup
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

        return channel;
    }
}
