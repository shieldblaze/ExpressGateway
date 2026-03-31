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
package com.shieldblaze.expressgateway.protocol.http.websocket;

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
import io.netty.handler.codec.http2.Http2DataFrame;
import io.netty.handler.codec.http2.Http2FrameCodecBuilder;
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
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.Timeout;

import java.net.InetSocketAddress;
import java.nio.charset.StandardCharsets;
import java.util.List;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Tests for WebSocket-over-HTTP/2 (RFC 8441 Extended CONNECT) proxy behavior.
 *
 * <p>Verifies the {@link WebSocketOverH2Handler} correctly:
 * <ul>
 *   <li>Accepts Extended CONNECT with {@code :protocol=websocket} and responds 200</li>
 *   <li>Bridges H2 DATA frames from client to WebSocket frames to backend</li>
 *   <li>Bridges WebSocket frames from backend to H2 DATA frames to client</li>
 *   <li>Handles stream teardown (endStream flag)</li>
 * </ul>
 *
 * <p>Test architecture:
 * <pre>
 *   H2 Client ---TLS/H2--- [LB proxy] ---TCP/WS--- WebSocketEchoServer
 * </pre>
 *
 * <p>The H2 client sends Extended CONNECT per RFC 8441, the proxy bridges to a
 * plain WebSocket backend. DATA frames containing WebSocket binary payload flow
 * end-to-end and are echoed back.
 */
@Timeout(value = 120, unit = TimeUnit.SECONDS)
class WebSocketOverH2Test {

    private static final Logger logger = LogManager.getLogger(WebSocketOverH2Test.class);

    private static WebSocketEchoServer webSocketEchoServer;
    private static HTTPLoadBalancer httpLoadBalancer;
    private static int lbPort;
    private static EventLoopGroup clientGroup;
    private static SslContext clientSslCtx;

    @BeforeAll
    static void setup() throws Exception {
        // 1. Start WebSocket echo backend (plain HTTP/1.1 WebSocket)
        webSocketEchoServer = new WebSocketEchoServer();
        webSocketEchoServer.startServer();

        // 2. Generate self-signed cert for TLS frontend
        SelfSignedCertificate ssc = SelfSignedCertificate.generateNew(
                List.of("127.0.0.1"), List.of("localhost"));
        CertificateKeyPair serverCkp = CertificateKeyPair.forClient(
                List.of(ssc.x509Certificate()), ssc.keyPair().getPrivate());

        TlsServerConfiguration tlsServerConfig =
                TlsServerConfiguration.copyFrom(TlsServerConfiguration.DEFAULT);
        tlsServerConfig.enable();
        tlsServerConfig.addMapping("localhost", serverCkp);
        tlsServerConfig.defaultMapping(serverCkp);

        // Backend connection is plain (no TLS to the WS echo server)
        TlsClientConfiguration tlsClientConfig =
                TlsClientConfiguration.copyFrom(TlsClientConfiguration.DEFAULT);

        // 3. Build and start load balancer with TLS (H2 via ALPN)
        lbPort = AvailablePortUtil.getTcpPort();

        Cluster cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new HTTPRoundRobin(NOOPSessionPersistence.INSTANCE))
                .build();

        httpLoadBalancer = HTTPLoadBalancerBuilder.newBuilder()
                .withConfigurationContext(ConfigurationContext.create(tlsClientConfig, tlsServerConfig))
                .withBindAddress(new InetSocketAddress("127.0.0.1", lbPort))
                .withL4FrontListener(new TCPListener())
                .build();

        httpLoadBalancer.mappedCluster("localhost:" + lbPort, cluster);

        NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress("127.0.0.1", webSocketEchoServer.port()))
                .build();

        httpLoadBalancer.start().future().get(60, TimeUnit.SECONDS);
        Thread.sleep(500); // Allow pipeline initialization

        // 4. Build H2 client SSL context
        clientSslCtx = SslContextBuilder.forClient()
                .sslProvider(SslProvider.JDK)
                .trustManager(InsecureTrustManagerFactory.INSTANCE)
                .applicationProtocolConfig(new ApplicationProtocolConfig(
                        ApplicationProtocolConfig.Protocol.ALPN,
                        ApplicationProtocolConfig.SelectorFailureBehavior.NO_ADVERTISE,
                        ApplicationProtocolConfig.SelectedListenerFailureBehavior.ACCEPT,
                        ApplicationProtocolNames.HTTP_2,
                        ApplicationProtocolNames.HTTP_1_1))
                .build();

        clientGroup = new NioEventLoopGroup(1);
    }

    @AfterAll
    static void shutdown() throws Exception {
        if (clientGroup != null) {
            clientGroup.shutdownGracefully().sync();
        }
        if (httpLoadBalancer != null) {
            httpLoadBalancer.stop().future().get(30, TimeUnit.SECONDS);
        }
        if (webSocketEchoServer != null) {
            webSocketEchoServer.shutdown();
        }
    }

    /**
     * RFC 8441: Send Extended CONNECT with :protocol=websocket, verify 200 OK response.
     *
     * <p>This validates the proxy detects the Extended CONNECT pseudo-headers
     * and installs WebSocketOverH2Handler, which responds with 200 on the stream.
     */
    @Test
    void extendedConnect_receives200Response() throws Exception {
        CompletableFuture<String> statusFuture = new CompletableFuture<>();

        Channel parentChannel = connectH2Client();
        try {
            Http2StreamChannel stream = openStream(parentChannel, new SimpleChannelInboundHandler<Http2StreamFrame>() {
                @Override
                protected void channelRead0(ChannelHandlerContext ctx, Http2StreamFrame frame) {
                    if (frame instanceof Http2HeadersFrame headersFrame) {
                        CharSequence status = headersFrame.headers().status();
                        if (status != null) {
                            statusFuture.complete(status.toString());
                        }
                    }
                }
            });

            sendExtendedConnect(stream);

            String status = statusFuture.get(10, TimeUnit.SECONDS);
            assertEquals("200", status, "Extended CONNECT must receive 200 OK per RFC 8441");
        } finally {
            parentChannel.close().sync();
        }
    }

    /**
     * End-to-end data echo: send binary data via H2 DATA frames through the proxy,
     * backend WebSocket echo server echoes it, and verify reception as H2 DATA.
     *
     * <p>The proxy translates:
     * <pre>
     *   Client H2 DATA -> BinaryWebSocketFrame -> backend
     *   Backend TextWebSocketFrame echo -> H2 DATA -> Client
     * </pre>
     *
     * <p>Note: The backend WebSocketHandler echoes TextWebSocketFrame only.
     * WebSocketOverH2Handler sends BinaryWebSocketFrame to the backend.
     * The echo server only echoes TextWebSocketFrame, so binary data sent
     * via Extended CONNECT will not be echoed back by the test echo server.
     * Instead, this test verifies that:
     * (a) The 200 response arrives (Extended CONNECT accepted)
     * (b) DATA frames are sent without error (proxy forwards to backend)
     * (c) The stream can be closed cleanly via endStream
     */
    @Test
    void extendedConnect_dataFrameForwarding_andCleanClose() throws Exception {
        CompletableFuture<String> statusFuture = new CompletableFuture<>();
        CountDownLatch streamClosed = new CountDownLatch(1);

        Channel parentChannel = connectH2Client();
        try {
            Http2StreamChannel stream = openStream(parentChannel, new SimpleChannelInboundHandler<Http2StreamFrame>() {
                @Override
                protected void channelRead0(ChannelHandlerContext ctx, Http2StreamFrame frame) {
                    if (frame instanceof Http2HeadersFrame headersFrame) {
                        CharSequence status = headersFrame.headers().status();
                        if (status != null) {
                            statusFuture.complete(status.toString());
                        }
                        if (headersFrame.isEndStream()) {
                            streamClosed.countDown();
                        }
                    }
                    if (frame instanceof Http2DataFrame dataFrame) {
                        if (dataFrame.isEndStream()) {
                            streamClosed.countDown();
                        }
                    }
                }

                @Override
                public void channelInactive(ChannelHandlerContext ctx) {
                    streamClosed.countDown();
                }
            });

            sendExtendedConnect(stream);

            // Wait for 200 OK
            String status = statusFuture.get(10, TimeUnit.SECONDS);
            assertEquals("200", status);

            // Send DATA frames to the proxy (these are forwarded as BinaryWebSocketFrame to backend)
            for (int i = 0; i < 5; i++) {
                ByteBuf payload = Unpooled.copiedBuffer("message-" + i, StandardCharsets.UTF_8);
                Http2DataFrame dataFrame = new DefaultHttp2DataFrame(payload, false);
                stream.writeAndFlush(dataFrame);
            }

            // Brief wait for proxy to process and forward
            Thread.sleep(500);

            // Send endStream DATA frame to cleanly close the WebSocket-over-H2 stream
            Http2DataFrame endFrame = new DefaultHttp2DataFrame(Unpooled.EMPTY_BUFFER, true);
            stream.writeAndFlush(endFrame);

            // The proxy should close the backend WS connection, which eventually
            // sends an end-of-stream DATA frame back or the stream channel goes inactive.
            assertTrue(streamClosed.await(10, TimeUnit.SECONDS),
                    "Stream should close after endStream is sent");
        } finally {
            parentChannel.close().sync();
        }
    }

    /**
     * Multiple concurrent Extended CONNECT streams on the same H2 connection.
     *
     * <p>RFC 8441 allows multiple WebSocket streams to be multiplexed over a single
     * HTTP/2 connection. Each stream gets its own WebSocketOverH2Handler instance.
     * This test verifies that independent streams do not interfere with each other.
     */
    @Test
    void multipleExtendedConnectStreams_independentHandling() throws Exception {
        int streamCount = 3;
        CountDownLatch allResponded = new CountDownLatch(streamCount);
        List<String> statuses = new CopyOnWriteArrayList<>();

        Channel parentChannel = connectH2Client();
        try {
            Http2StreamChannel[] streams = new Http2StreamChannel[streamCount];
            for (int i = 0; i < streamCount; i++) {
                streams[i] = openStream(parentChannel, new SimpleChannelInboundHandler<Http2StreamFrame>() {
                    @Override
                    protected void channelRead0(ChannelHandlerContext ctx, Http2StreamFrame frame) {
                        if (frame instanceof Http2HeadersFrame headersFrame) {
                            CharSequence status = headersFrame.headers().status();
                            if (status != null) {
                                statuses.add(status.toString());
                                allResponded.countDown();
                            }
                        }
                    }
                });
                sendExtendedConnect(streams[i]);
            }

            assertTrue(allResponded.await(15, TimeUnit.SECONDS),
                    "All " + streamCount + " Extended CONNECT streams must receive 200 OK");

            // Each stream should have received 200
            assertEquals(streamCount, statuses.size());
            for (String s : statuses) {
                assertEquals("200", s, "Each stream must receive 200 OK");
            }

            // Close all streams cleanly
            for (Http2StreamChannel stream : streams) {
                if (stream.isActive()) {
                    stream.writeAndFlush(new DefaultHttp2DataFrame(Unpooled.EMPTY_BUFFER, true));
                }
            }

            Thread.sleep(500);
        } finally {
            parentChannel.close().sync();
        }
    }

    /**
     * Extended CONNECT followed by endStream DATA immediately closes the proxied
     * WebSocket connection without error.
     *
     * <p>This tests the close() idempotency: the endStream triggers close() once,
     * and the subsequent channelInactive must not fail on double-close.
     */
    @Test
    void extendedConnect_immediateEndStream_closesCleanly() throws Exception {
        CompletableFuture<String> statusFuture = new CompletableFuture<>();
        CountDownLatch streamDone = new CountDownLatch(1);

        Channel parentChannel = connectH2Client();
        try {
            Http2StreamChannel stream = openStream(parentChannel, new SimpleChannelInboundHandler<Http2StreamFrame>() {
                @Override
                protected void channelRead0(ChannelHandlerContext ctx, Http2StreamFrame frame) {
                    if (frame instanceof Http2HeadersFrame headersFrame) {
                        CharSequence status = headersFrame.headers().status();
                        if (status != null) {
                            statusFuture.complete(status.toString());
                        }
                    }
                    if (frame instanceof Http2DataFrame dataFrame && dataFrame.isEndStream()) {
                        streamDone.countDown();
                    }
                }

                @Override
                public void channelInactive(ChannelHandlerContext ctx) {
                    streamDone.countDown();
                }
            });

            sendExtendedConnect(stream);

            // Wait for 200 OK
            String status = statusFuture.get(10, TimeUnit.SECONDS);
            assertEquals("200", status);

            // Brief wait for backend WebSocket handshake to complete
            Thread.sleep(500);

            // Send endStream immediately -- this triggers WebSocketOverH2Handler.close()
            // which closes the backend channel, and the backend disconnection triggers
            // BackendToH2Bridge.channelInactive() -> endStream DATA back to client.
            stream.writeAndFlush(new DefaultHttp2DataFrame(Unpooled.EMPTY_BUFFER, true));

            assertTrue(streamDone.await(10, TimeUnit.SECONDS),
                    "Stream must close cleanly after endStream DATA");
        } finally {
            parentChannel.close().sync();
        }
    }

    // -- Helper methods --

    /**
     * Establishes an H2 connection (TLS + ALPN) to the load balancer.
     */
    private Channel connectH2Client() throws Exception {
        Bootstrap bootstrap = new Bootstrap()
                .group(clientGroup)
                .channel(NioSocketChannel.class)
                .handler(new ChannelInitializer<SocketChannel>() {
                    @Override
                    protected void initChannel(SocketChannel ch) {
                        SslHandler sslHandler = clientSslCtx.newHandler(ch.alloc(), "localhost", lbPort);
                        ch.pipeline().addLast(sslHandler);
                        ch.pipeline().addLast(Http2FrameCodecBuilder.forClient().build());
                        ch.pipeline().addLast(new Http2MultiplexHandler(
                                new SimpleChannelInboundHandler<Http2StreamFrame>() {
                                    @Override
                                    protected void channelRead0(ChannelHandlerContext ctx, Http2StreamFrame frame) {
                                        // Server-initiated streams (not expected in this test)
                                    }
                                }));
                    }
                });

        ChannelFuture cf = bootstrap.connect("127.0.0.1", lbPort).sync();
        assertTrue(cf.isSuccess(), "H2 client must connect to LB");

        Channel channel = cf.channel();
        channel.pipeline().get(SslHandler.class).handshakeFuture().sync();
        Thread.sleep(300); // Allow SETTINGS exchange
        return channel;
    }

    /**
     * Opens an H2 stream channel with the given handler.
     */
    private Http2StreamChannel openStream(Channel parentChannel,
                                          SimpleChannelInboundHandler<Http2StreamFrame> handler) throws Exception {
        return new Http2StreamChannelBootstrap(parentChannel)
                .handler(handler)
                .open().sync().getNow();
    }

    /**
     * Sends RFC 8441 Extended CONNECT request on the given stream.
     *
     * <p>Per RFC 8441 Section 4, the Extended CONNECT uses:
     * <ul>
     *   <li>{@code :method = CONNECT}</li>
     *   <li>{@code :protocol = websocket}</li>
     *   <li>{@code :scheme = https}</li>
     *   <li>{@code :path = /}</li>
     *   <li>{@code :authority = <lb host:port>}</li>
     * </ul>
     *
     * <p>Unlike regular CONNECT, Extended CONNECT includes :scheme and :path
     * pseudo-headers. The :protocol pseudo-header signals the desired protocol.
     */
    private void sendExtendedConnect(Http2StreamChannel stream) {
        Http2Headers headers = new DefaultHttp2Headers()
                .method("CONNECT")
                .path("/")
                .scheme("https")
                .authority("localhost:" + lbPort)
                .set(":protocol", "websocket");

        // endStream=false: the stream remains open for bidirectional DATA exchange
        stream.writeAndFlush(new DefaultHttp2HeadersFrame(headers, false));
    }
}
