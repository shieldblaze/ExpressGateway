/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020 ShieldBlaze
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
package com.shieldblaze.expressgateway.core.server.http.pool;

import com.shieldblaze.expressgateway.backend.Backend;
import com.shieldblaze.expressgateway.backend.State;
import com.shieldblaze.expressgateway.backend.connection.Bootstrapper;
import com.shieldblaze.expressgateway.backend.connection.ClusterConnectionPool;
import com.shieldblaze.expressgateway.backend.connection.Connection;
import com.shieldblaze.expressgateway.backend.connection.TooManyConnectionsException;
import com.shieldblaze.expressgateway.backend.exceptions.BackendNotAvailableException;
import com.shieldblaze.expressgateway.backend.exceptions.LoadBalanceException;
import com.shieldblaze.expressgateway.backend.loadbalance.Request;
import com.shieldblaze.expressgateway.backend.loadbalance.Response;
import com.shieldblaze.expressgateway.configuration.http.HTTPConfiguration;
import com.shieldblaze.expressgateway.core.loadbalancer.l7.http.HTTPLoadBalancer;
import com.shieldblaze.expressgateway.core.server.http.HTTPUtils;
import com.shieldblaze.expressgateway.core.server.http.alpn.ALPNHandler;
import com.shieldblaze.expressgateway.core.server.http.alpn.ALPNHandlerBuilder;
import com.shieldblaze.expressgateway.core.server.http.compression.HTTPContentDecompressor;
import com.shieldblaze.expressgateway.core.server.http.http2.HTTP2ToHTTP1Adapter;
import com.shieldblaze.expressgateway.loadbalance.l7.http.HTTPBalanceRequest;
import com.shieldblaze.expressgateway.loadbalance.l7.http.HTTPBalanceResponse;
import io.netty.bootstrap.Bootstrap;
import io.netty.channel.ChannelHandler;
import io.netty.channel.ChannelInitializer;
import io.netty.channel.ChannelPipeline;
import io.netty.channel.socket.SocketChannel;
import io.netty.handler.ssl.SslHandler;
import io.netty.handler.timeout.IdleStateHandler;

import java.util.Objects;

public class HTTPClusterConnectionPool extends ClusterConnectionPool {

    private HTTPLoadBalancer httpLoadBalancer;

    public HTTPClusterConnectionPool(Bootstrapper bootstrapper) {
        super(bootstrapper);
    }

    public HTTPBalanceResponse getResponse(HTTPBalanceRequest request) throws LoadBalanceException {
        return (HTTPBalanceResponse) super.getResponse(request);
    }

    @Override
    public HTTPBalanceResponse getResponse(Request request) throws LoadBalanceException {
        return (HTTPBalanceResponse) super.getResponse(request);
    }

    @Override
    public HTTPConnection acquireConnection(Response response, ChannelHandler downstreamHandler) throws BackendNotAvailableException {
        Backend backend = response.getBackend();

        if (backend.getState() != State.ONLINE) {
            throw new BackendNotAvailableException("Backend: " + backend.getSocketAddress() + " is not online");
        }

        HTTPConnection connection = (HTTPConnection) backendAvailableConnectionMap.get(backend).poll();

        // If connection is available, return it.
        if (connection != null) {
            return connection;
        }

        // Check for connection limit before creating new connection.
        if (backendAvailableConnectionMap.size() > backend.getMaxConnections()) {
            // Throw exception because we have too many active connections
            throw new TooManyConnectionsException(backend);
        }

        backend.incConnections(); // Increment number of connections in Backend

        // Create a new HTTP connection
        HTTPConnection httpConnection = new HTTPConnection();
        backendActiveConnectionMap.get(backend).add(httpConnection);
        Bootstrap bootstrap = bootstrapper.bootstrap();
        bootstrap.handler(new ChannelInitializer<SocketChannel>() {
            @Override
            protected void initChannel(SocketChannel ch) {
                ChannelPipeline pipeline = ch.pipeline();

                HTTPConfiguration httpConfiguration = httpLoadBalancer.getHTTPConfiguration();

                int timeout = httpLoadBalancer.getCommonConfiguration().getTransportConfiguration().getConnectionIdleTimeout();
                pipeline.addFirst("IdleStateHandler", new IdleStateHandler(timeout, timeout, timeout));

                if (httpLoadBalancer.getTlsClient() == null) {
                    pipeline.addLast("HTTPClientCodec", HTTPUtils.newClientCodec(httpConfiguration));
                    pipeline.addLast("HTTPContentDecompressor", new HTTPContentDecompressor());
                    pipeline.addLast("DownstreamHandler", downstreamHandler);
                } else {
                    String hostname = backend.getSocketAddress().getHostName();
                    int port = backend.getSocketAddress().getPort();
                    SslHandler sslHandler = httpLoadBalancer.getTlsClient().getDefault().getSslContext().newHandler(ch.alloc(), hostname, port);

                    ALPNHandler alpnHandler = ALPNHandlerBuilder.newBuilder()
                            // HTTP/2 Handlers
                            .withHTTP2ChannelHandler("HTTP2Handler", HTTPUtils.clientH2Handler(httpConfiguration))
                            .withHTTP2ChannelHandler("DownstreamHandler", downstreamHandler)
                            // HTTP/1.1 Handlers
                            .withHTTP1ChannelHandler("HTTPClientCodec", HTTPUtils.newClientCodec(httpConfiguration))
                            .withHTTP1ChannelHandler("HTTPContentDecompressor", new HTTPContentDecompressor())
                            .withHTTP1ChannelHandler("DownstreamHandler", downstreamHandler)
                            .build();

                    pipeline.addLast("TLSHandler", sslHandler);
                    pipeline.addLast("ALPNHandler", alpnHandler);

                    httpConnection.setALPNHandlerPresent();
                }
            }
        });

        httpConnection.setChannelFuture(bootstrap.connect(backend.getSocketAddress()));
        return httpConnection;
    }

    public void setHTTPLoadBalancer(HTTPLoadBalancer httpLoadBalancer) {
        if (this.httpLoadBalancer == null) {
            this.httpLoadBalancer = Objects.requireNonNull(httpLoadBalancer, "HTTPLoadBalancer");
            bootstrapper.setAllocator(httpLoadBalancer.getByteBufAllocator());
            bootstrapper.setCommonConfiguration(httpLoadBalancer.getCommonConfiguration());
            bootstrapper.setEventLoopFactory(httpLoadBalancer.getEventLoopFactory().getChildGroup());
        } else {
            throw new IllegalArgumentException("HTTPLoadBalancer is already set");
        }
    }
}
