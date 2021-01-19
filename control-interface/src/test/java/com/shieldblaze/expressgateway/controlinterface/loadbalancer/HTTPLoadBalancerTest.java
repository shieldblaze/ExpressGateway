/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2021 ShieldBlaze
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
package com.shieldblaze.expressgateway.controlinterface.loadbalancer;

import com.shieldblaze.expressgateway.controlinterface.node.NodeOuterClass;
import com.shieldblaze.expressgateway.controlinterface.node.NodeService;
import com.shieldblaze.expressgateway.controlinterface.node.NodeServiceGrpc;
import com.shieldblaze.expressgateway.protocol.http.alpn.ALPNHandler;
import com.shieldblaze.expressgateway.protocol.http.alpn.ALPNHandlerBuilder;
import io.grpc.ManagedChannel;
import io.grpc.ManagedChannelBuilder;
import io.grpc.Server;
import io.grpc.netty.shaded.io.grpc.netty.NettyServerBuilder;
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
import io.netty.handler.codec.http2.DefaultHttp2Connection;
import io.netty.handler.codec.http2.Http2Connection;
import io.netty.handler.codec.http2.HttpConversionUtil;
import io.netty.handler.codec.http2.HttpToHttp2ConnectionHandler;
import io.netty.handler.codec.http2.HttpToHttp2ConnectionHandlerBuilder;
import io.netty.handler.codec.http2.InboundHttp2ToHttpAdapter;
import io.netty.handler.codec.http2.InboundHttp2ToHttpAdapterBuilder;
import io.netty.handler.ssl.ApplicationProtocolConfig;
import io.netty.handler.ssl.ApplicationProtocolNames;
import io.netty.handler.ssl.SslContext;
import io.netty.handler.ssl.SslContextBuilder;
import io.netty.handler.ssl.SslProvider;
import io.netty.handler.ssl.util.SelfSignedCertificate;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.MethodOrderer;
import org.junit.jupiter.api.Order;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.TestMethodOrder;

import java.io.IOException;
import java.net.InetSocketAddress;
import java.net.ServerSocket;
import java.net.Socket;
import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.util.Objects;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

@TestMethodOrder(MethodOrderer.OrderAnnotation.class)
class HTTPLoadBalancerTest {

    static Server server;
    static ManagedChannel channel;
    static String loadBalancerId;

    @BeforeAll
    static void setup() throws IOException {
        System.setProperty("EGWConfDir", System.getProperty("java.io.tmpdir"));

        server = NettyServerBuilder.forAddress(new InetSocketAddress("127.0.0.1", 9110))
                .addService(new TCPLoadBalancerService())
                .addService(new NodeService())
                .build()
                .start();

        channel = ManagedChannelBuilder.forTarget("127.0.0.1:9110")
                .usePlaintext()
                .build();
    }

    @AfterAll
    static void shutdown() throws InterruptedException {
        channel.shutdownNow();
        server.shutdownNow().awaitTermination();
    }

    @Test
    @Order(1)
    void simpleServerLBClientTest() throws IOException, InterruptedException {
        new HTTPServer(5555, false).start();

        TCPLoadBalancerServiceGrpc.TCPLoadBalancerServiceBlockingStub tcpService = TCPLoadBalancerServiceGrpc.newBlockingStub(channel);
        NodeServiceGrpc.NodeServiceBlockingStub nodeService = NodeServiceGrpc.newBlockingStub(channel);

        LoadBalancer.TCPLoadBalancer tcpLoadBalancer = LoadBalancer.TCPLoadBalancer.newBuilder()
                .setBindAddress("127.0.0.1")
                .setBindPort(5000)
                .setName("Meow")
                .setUseDefaults(true)
                .setLayer7(LoadBalancer.Layer7.HTTP)
                .build();

        LoadBalancer.LoadBalancerResponse loadBalancerResponse = tcpService.start(tcpLoadBalancer);
        assertFalse(loadBalancerResponse.getResponseText().isEmpty()); // Load Balancer ID must exist

        loadBalancerId = loadBalancerResponse.getResponseText();

        NodeOuterClass.AddRequest addRequest = NodeOuterClass.AddRequest.newBuilder()
                .setAddress("127.0.0.1")
                .setPort(5555)
                .setLoadBalancerID(loadBalancerResponse.getResponseText())
                .setMaxConnections(-1)
                .build();

        NodeOuterClass.AddResponse addResponse = nodeService.add(addRequest);
        assertTrue(addResponse.getSuccess());
        assertFalse(addResponse.getNodeId().isEmpty()); // Load Balancer ID

        HttpClient httpClient = HttpClient.newHttpClient();
        HttpRequest httpRequest = HttpRequest.newBuilder()
                .uri(URI.create("http://127.0.0.1:5000"))
                .GET()
                .build();

        HttpResponse<Void> response = httpClient.send(httpRequest, java.net.http.HttpResponse.BodyHandlers.discarding());
        assertEquals(200, response.statusCode());

        Thread.sleep(2500L); // Wait for everything to settle down
    }

    @Test
    @Order(2)
    void getTest() {
        TCPLoadBalancerServiceGrpc.TCPLoadBalancerServiceBlockingStub tcpService = TCPLoadBalancerServiceGrpc.newBlockingStub(channel);
        LoadBalancer.GetLoadBalancerRequest request = LoadBalancer.GetLoadBalancerRequest.newBuilder()
                .setLoadBalancerId(loadBalancerId)
                .build();

        LoadBalancer.TCPLoadBalancer tcpLoadBalancer = tcpService.get(request);

        assertEquals("127.0.0.1", tcpLoadBalancer.getBindAddress());
        assertEquals(5000, tcpLoadBalancer.getBindPort());
    }

    @Test
    @Order(3)
    void stopTest() {
        TCPLoadBalancerServiceGrpc.TCPLoadBalancerServiceBlockingStub tcpService = TCPLoadBalancerServiceGrpc.newBlockingStub(channel);
        LoadBalancer.LoadBalancerResponse response = tcpService.stop(LoadBalancer.StopLoadBalancer.newBuilder().setId(loadBalancerId).build());
        assertEquals("Success", response.getResponseText());
    }

    private static final class HTTPServer extends Thread {

        private final int port;
        private final boolean tls;
        private final ChannelHandler channelHandler;
        private EventLoopGroup eventLoopGroup;
        private ChannelFuture channelFuture;

        HTTPServer(int port, boolean tls) {
            this(port, tls, null);
        }

        HTTPServer(int port, boolean tls, ChannelHandler channelHandler) {
            this.port = port;
            this.tls = tls;
            this.channelHandler = Objects.requireNonNullElseGet(channelHandler, Handler::new);
        }

        @Override
        public void run() {

            try {

                eventLoopGroup = new NioEventLoopGroup(2);

                SelfSignedCertificate selfSignedCertificate = new SelfSignedCertificate("localhost", "EC", 256);
                SslContext sslContext = SslContextBuilder.forServer(selfSignedCertificate.certificate(), selfSignedCertificate.privateKey())
                        .sslProvider(SslProvider.JDK)
                        .applicationProtocolConfig(new ApplicationProtocolConfig(
                                ApplicationProtocolConfig.Protocol.ALPN,
                                ApplicationProtocolConfig.SelectorFailureBehavior.NO_ADVERTISE,
                                ApplicationProtocolConfig.SelectedListenerFailureBehavior.ACCEPT,
                                ApplicationProtocolNames.HTTP_2,
                                ApplicationProtocolNames.HTTP_1_1))
                        .build();

                ServerBootstrap serverBootstrap = new ServerBootstrap()
                        .group(eventLoopGroup, eventLoopGroup)
                        .channel(NioServerSocketChannel.class)
                        .childHandler(new ChannelInitializer<SocketChannel>() {
                            @Override
                            protected void initChannel(SocketChannel ch) {

                                if (tls) {
                                    ch.pipeline().addLast(sslContext.newHandler(ch.alloc()));

                                    Http2Connection http2Connection = new DefaultHttp2Connection(true);

                                    InboundHttp2ToHttpAdapter adapter = new InboundHttp2ToHttpAdapterBuilder(http2Connection)
                                            .propagateSettings(false)
                                            .maxContentLength(Integer.MAX_VALUE)
                                            .validateHttpHeaders(true)
                                            .build();

                                    HttpToHttp2ConnectionHandler httpToHttp2ConnectionHandler = new HttpToHttp2ConnectionHandlerBuilder()
                                            .connection(http2Connection)
                                            .frameListener(adapter)
                                            .build();

                                    ALPNHandler alpnHandler = ALPNHandlerBuilder.newBuilder()
                                            .withHTTP2ChannelHandler(httpToHttp2ConnectionHandler)
                                            .withHTTP2ChannelHandler(channelHandler)
                                            .withHTTP1ChannelHandler(new HttpServerCodec())
                                            .withHTTP1ChannelHandler(new HttpObjectAggregator(Integer.MAX_VALUE))
                                            .withHTTP1ChannelHandler(channelHandler)
                                            .build();

                                    ch.pipeline().addLast(alpnHandler);
                                } else {
                                    ch.pipeline().addLast(new HttpServerCodec());
                                    ch.pipeline().addLast(new HttpObjectAggregator(Integer.MAX_VALUE));
                                    ch.pipeline().addLast(channelHandler);
                                }
                            }
                        });

                channelFuture = serverBootstrap.bind("127.0.0.1", port);
            } catch (Exception ex) {
                ex.printStackTrace();
            }
        }

        public void shutdown() {
            channelFuture.channel().close();
            eventLoopGroup.shutdownGracefully();
        }

        @ChannelHandler.Sharable
        private static final class Handler extends SimpleChannelInboundHandler<FullHttpRequest> {

            @Override
            protected void channelRead0(ChannelHandlerContext ctx, FullHttpRequest msg) {
                DefaultFullHttpResponse httpResponse = new DefaultFullHttpResponse(HttpVersion.HTTP_1_1, HttpResponseStatus.OK, Unpooled.wrappedBuffer("Meow".getBytes()));
                if (msg.headers().contains(HttpConversionUtil.ExtensionHeaderNames.STREAM_ID.text())) {
                    httpResponse.headers().set("x-http2-stream-id", msg.headers().get(HttpConversionUtil.ExtensionHeaderNames.STREAM_ID.text()));
                } else {
                    httpResponse.headers().set(HttpHeaderNames.CONTENT_LENGTH, 4);
                }
                ctx.writeAndFlush(httpResponse);
            }
        }
    }
}
