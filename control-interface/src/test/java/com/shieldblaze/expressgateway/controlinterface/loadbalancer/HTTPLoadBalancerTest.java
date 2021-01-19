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
import io.netty.handler.codec.http2.HttpConversionUtil;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.MethodOrderer;
import org.junit.jupiter.api.Order;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.TestMethodOrder;

import java.io.IOException;
import java.net.InetSocketAddress;
import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.util.concurrent.TimeUnit;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

@TestMethodOrder(MethodOrderer.OrderAnnotation.class)
class HTTPLoadBalancerTest {

    static Server server;
    static ManagedChannel channel;
    static String loadBalancerId;
    static HTTPServer httpServer = new HTTPServer();

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
        httpServer.shutdown();
        channel.shutdownNow();
        server.shutdownNow().awaitTermination(30, TimeUnit.SECONDS);
    }

    @Test
    @Order(1)
    void simpleServerLBClientTest() throws IOException, InterruptedException {
        httpServer.start();

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

        private EventLoopGroup eventLoopGroup;
        private ChannelFuture channelFuture;

        @Override
        public void run() {
            try {
                eventLoopGroup = new NioEventLoopGroup(2);

                ServerBootstrap serverBootstrap = new ServerBootstrap()
                        .group(eventLoopGroup, eventLoopGroup)
                        .channel(NioServerSocketChannel.class)
                        .childHandler(new ChannelInitializer<SocketChannel>() {
                            @Override
                            protected void initChannel(SocketChannel ch) {
                                ch.pipeline().addLast(new HttpServerCodec());
                                ch.pipeline().addLast(new HttpObjectAggregator(Integer.MAX_VALUE));
                                ch.pipeline().addLast(new Handler());
                            }
                        });

                channelFuture = serverBootstrap.bind("127.0.0.1", 5555);
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
