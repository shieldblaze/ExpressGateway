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

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.NodeBytesTracker;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.HTTPBalanceRequest;
import com.shieldblaze.expressgateway.core.factory.BootstrapFactory;
import com.shieldblaze.expressgateway.core.handlers.ConnectionTimeoutHandler;
import com.shieldblaze.expressgateway.protocol.http.Headers;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancer;
import io.netty.bootstrap.Bootstrap;
import io.netty.buffer.ByteBuf;
import io.netty.channel.Channel;
import io.netty.channel.ChannelFuture;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInboundHandlerAdapter;
import io.netty.channel.ChannelInitializer;
import io.netty.channel.ChannelPipeline;
import io.netty.channel.socket.SocketChannel;
import io.netty.handler.codec.http.DefaultHttpHeaders;
import io.netty.handler.codec.http.HttpClientCodec;
import io.netty.handler.codec.http.HttpHeaders;
import io.netty.handler.codec.http.HttpObjectAggregator;
import io.netty.handler.codec.http.websocketx.BinaryWebSocketFrame;
import io.netty.handler.codec.http.websocketx.WebSocketClientProtocolHandler;
import io.netty.handler.codec.http.websocketx.WebSocketFrame;
import io.netty.handler.codec.http2.DefaultHttp2DataFrame;
import io.netty.handler.codec.http2.DefaultHttp2Headers;
import io.netty.handler.codec.http2.DefaultHttp2HeadersFrame;
import io.netty.handler.codec.http2.Http2DataFrame;
import io.netty.handler.codec.http2.Http2FrameStream;
import io.netty.handler.codec.http2.Http2Headers;
import io.netty.handler.codec.http2.Http2HeadersFrame;
import io.netty.util.ReferenceCountUtil;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.io.Closeable;
import java.io.IOException;
import java.net.InetSocketAddress;
import java.net.URI;
import java.time.Duration;

import static io.netty.handler.codec.http.websocketx.WebSocketVersion.V13;

/**
 * Bridges WebSocket-over-HTTP/2 (RFC 8441) Extended CONNECT streams to
 * WebSocket backend connections.
 *
 * <p>RFC 8441 defines how WebSocket connections can be established over HTTP/2
 * using the Extended CONNECT method with the {@code :protocol} pseudo-header
 * set to {@code "websocket"}. This handler:</p>
 *
 * <ol>
 *   <li>Receives the Extended CONNECT HEADERS frame from the H2 frontend</li>
 *   <li>Sends a 200 OK response back on the H2 stream</li>
 *   <li>Establishes a WebSocket connection to the backend using the
 *       existing WebSocket bootstrapping infrastructure</li>
 *   <li>Bridges H2 DATA frames from client to WebSocket frames to backend,
 *       and WebSocket frames from backend to H2 DATA frames to client</li>
 * </ol>
 *
 * <p>This handler is installed into the frontend H2 pipeline by
 * {@code Http2ServerInboundHandler} when it detects an Extended CONNECT
 * request with {@code :protocol = websocket}.</p>
 */
public final class WebSocketOverH2Handler extends ChannelInboundHandlerAdapter implements Closeable {

    private static final Logger logger = LogManager.getLogger(WebSocketOverH2Handler.class);

    private final HTTPLoadBalancer httpLoadBalancer;
    private final Http2FrameStream clientStream;
    private final String authority;
    private final String path;
    private final InetSocketAddress clientAddress;

    private volatile Channel backendChannel;
    private ChannelHandlerContext frontendCtx;
    private volatile boolean closed;

    /**
     * @param httpLoadBalancer the load balancer for backend selection
     * @param clientStream     the H2 stream for this WebSocket connection
     * @param authority        the :authority pseudo-header value
     * @param path             the :path pseudo-header value
     * @param clientAddress    the client's address
     */
    public WebSocketOverH2Handler(HTTPLoadBalancer httpLoadBalancer, Http2FrameStream clientStream,
                                   String authority, String path, InetSocketAddress clientAddress) {
        this.httpLoadBalancer = httpLoadBalancer;
        this.clientStream = clientStream;
        this.authority = authority;
        this.path = path;
        this.clientAddress = clientAddress;
    }

    @Override
    public void handlerAdded(ChannelHandlerContext ctx) throws Exception {
        this.frontendCtx = ctx;

        // Select a backend node
        Cluster cluster = httpLoadBalancer.cluster(authority);
        if (cluster == null) {
            logger.error("WebSocket over H2: no cluster found for authority '{}'", authority);
            sendErrorAndClose(ctx, "502");
            return;
        }

        Node node;
        try {
            node = cluster.nextNode(new HTTPBalanceRequest(clientAddress, new DefaultHttp2Headers().authority(authority))).node();
        } catch (Exception ex) {
            logger.error("WebSocket over H2: failed to select backend node", ex);
            sendErrorAndClose(ctx, "502");
            return;
        }

        // Send 200 OK response on the H2 stream to accept the Extended CONNECT
        Http2Headers responseHeaders = new DefaultHttp2Headers();
        responseHeaders.status("200");
        Http2HeadersFrame responseFrame = new DefaultHttp2HeadersFrame(responseHeaders, false);
        responseFrame.stream(clientStream);
        ctx.writeAndFlush(responseFrame);

        // Construct the WebSocket URI for the backend connection
        String scheme = httpLoadBalancer.configurationContext().tlsClientConfiguration().enabled() ? "wss" : "ws";
        URI wsUri = URI.create(scheme + "://" + node.socketAddress().getHostName() + ":" +
                node.socketAddress().getPort() + path);

        // Connect to backend WebSocket
        connectToBackend(ctx, node, wsUri);
    }

    private void connectToBackend(ChannelHandlerContext ctx, Node node, URI wsUri) {
        HttpHeaders headers = new DefaultHttpHeaders();
        headers.set(Headers.X_FORWARDED_FOR, clientAddress.getAddress().getHostAddress());

        Bootstrap bootstrap = BootstrapFactory.tcp(
                httpLoadBalancer.configurationContext(),
                httpLoadBalancer.eventLoopFactory().childGroup(),
                httpLoadBalancer.byteBufAllocator()
        );

        bootstrap.handler(new ChannelInitializer<SocketChannel>() {
            @Override
            protected void initChannel(SocketChannel ch) {
                ChannelPipeline pipeline = ch.pipeline();
                pipeline.addFirst(new NodeBytesTracker(node));

                Duration timeout = Duration.ofMillis(httpLoadBalancer.configurationContext().transportConfiguration().connectionIdleTimeout());
                pipeline.addLast(new ConnectionTimeoutHandler(timeout, false));

                if (httpLoadBalancer.configurationContext().tlsClientConfiguration().enabled()) {
                    pipeline.addLast(httpLoadBalancer.configurationContext().tlsClientConfiguration()
                            .defaultMapping().sslContext()
                            .newHandler(ch.alloc(), node.socketAddress().getHostName(), node.socketAddress().getPort()));
                }

                pipeline.addLast(new HttpClientCodec(
                        httpLoadBalancer.httpConfiguration().maxInitialLineLength(),
                        httpLoadBalancer.httpConfiguration().maxHeaderSize(),
                        httpLoadBalancer.httpConfiguration().maxChunkSize()
                ));
                pipeline.addLast(new HttpObjectAggregator(8192));
                pipeline.addLast(new WebSocketClientProtocolHandler(wsUri, V13, null, true, headers, 65536));

                // Handler that forwards WebSocket frames from backend -> H2 DATA frames to client
                pipeline.addLast(new BackendToH2Bridge(ctx, clientStream));
            }
        });

        ChannelFuture connectFuture = bootstrap.connect(node.socketAddress());
        connectFuture.addListener(future -> {
            if (future.isSuccess()) {
                backendChannel = connectFuture.channel();
                logger.debug("WebSocket over H2: connected to backend {} for stream {}",
                        node.socketAddress(), clientStream.id());
            } else {
                logger.error("WebSocket over H2: failed to connect to backend {}", node.socketAddress(), future.cause());
                sendEndStream(ctx);
            }
        });
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) throws Exception {
        if (msg instanceof Http2DataFrame dataFrame) {
            if (dataFrame.stream().id() == clientStream.id()) {
                try {
                    // Forward H2 DATA from client -> WebSocket binary frame to backend.
                    // CRIT-03: Only retain+write when the backend channel is confirmed active.
                    // If bc is null or inactive, skip the write — the try-finally ensures the
                    // dataFrame is always released, preventing ByteBuf leaks.
                    Channel bc = backendChannel;
                    if (bc != null && bc.isActive()) {
                        bc.writeAndFlush(new BinaryWebSocketFrame(dataFrame.content().retain()));
                    }

                    if (dataFrame.isEndStream()) {
                        close();
                    }
                } finally {
                    // CRIT-03: Always release the inbound dataFrame regardless of write outcome.
                    ReferenceCountUtil.release(dataFrame);
                }
                return;
            }
        }

        // Not for our stream, pass through
        ctx.fireChannelRead(msg);
    }

    private void sendErrorAndClose(ChannelHandlerContext ctx, String statusCode) {
        Http2Headers headers = new DefaultHttp2Headers();
        headers.status(statusCode);
        Http2HeadersFrame frame = new DefaultHttp2HeadersFrame(headers, true);
        frame.stream(clientStream);
        ctx.writeAndFlush(frame);
    }

    private void sendEndStream(ChannelHandlerContext ctx) {
        Http2DataFrame endFrame = new DefaultHttp2DataFrame(true);
        endFrame.stream(clientStream);
        ctx.writeAndFlush(endFrame);
    }

    @Override
    public void channelInactive(ChannelHandlerContext ctx) {
        close();
        ctx.fireChannelInactive();
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
        if (!(cause instanceof IOException)) {
            logger.error("WebSocket over H2: error on stream {}", clientStream.id(), cause);
        }
        close();
    }

    @Override
    public synchronized void close() {
        if (closed) {
            return;
        }
        closed = true;

        if (backendChannel != null && backendChannel.isActive()) {
            backendChannel.close();
        }
        backendChannel = null;
    }

    /**
     * Handler installed in the backend pipeline that bridges WebSocket frames
     * from the backend back to H2 DATA frames on the frontend stream.
     */
    private static final class BackendToH2Bridge extends ChannelInboundHandlerAdapter {

        private final ChannelHandlerContext frontendCtx;
        private final Http2FrameStream clientStream;

        BackendToH2Bridge(ChannelHandlerContext frontendCtx, Http2FrameStream clientStream) {
            this.frontendCtx = frontendCtx;
            this.clientStream = clientStream;
        }

        @Override
        public void channelRead(ChannelHandlerContext ctx, Object msg) {
            if (msg instanceof WebSocketFrame wsFrame) {
                // HI-05: Check if the frontend channel is still active before retaining.
                // If the frontend is gone, there's nowhere to send the data — just release.
                if (frontendCtx.channel().isActive()) {
                    ByteBuf content = wsFrame.content().retain();
                    Http2DataFrame dataFrame = new DefaultHttp2DataFrame(content, false);
                    dataFrame.stream(clientStream);
                    frontendCtx.writeAndFlush(dataFrame);
                }
                ReferenceCountUtil.release(wsFrame);
            } else {
                ReferenceCountUtil.release(msg);
            }
        }

        @Override
        public void channelInactive(ChannelHandlerContext ctx) {
            // Backend disconnected -- send end-of-stream to client
            Http2DataFrame endFrame = new DefaultHttp2DataFrame(true);
            endFrame.stream(clientStream);
            frontendCtx.writeAndFlush(endFrame);
        }

        @Override
        public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
            if (!(cause instanceof IOException)) {
                LogManager.getLogger(BackendToH2Bridge.class)
                        .error("WebSocket over H2 backend bridge error", cause);
            }
            ctx.close();
        }
    }
}
