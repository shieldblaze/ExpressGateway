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
package com.shieldblaze.expressgateway.protocol.tcp;

import com.shieldblaze.expressgateway.configuration.transport.ProxyProtocolMode;
import com.shieldblaze.expressgateway.core.handlers.ConnectionTimeoutHandler;
import com.shieldblaze.expressgateway.core.handlers.ProxyProtocolHandler;
import com.shieldblaze.expressgateway.core.handlers.SNIHandler;
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancer;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInitializer;
import io.netty.channel.ChannelPipeline;
import io.netty.channel.group.ChannelGroup;
import io.netty.channel.socket.SocketChannel;
import lombok.extern.log4j.Log4j2;

import java.time.Duration;

@Log4j2
final class ServerInitializer extends ChannelInitializer<SocketChannel> {

    private final L4LoadBalancer l4LoadBalancer;
    private final ChannelGroup activeConnections;

    ServerInitializer(L4LoadBalancer l4LoadBalancer) {
        this(l4LoadBalancer, null);
    }

    ServerInitializer(L4LoadBalancer l4LoadBalancer, ChannelGroup activeConnections) {
        this.l4LoadBalancer = l4LoadBalancer;
        this.activeConnections = activeConnections;
    }

    @Override
    protected void initChannel(SocketChannel ch) {
        ChannelPipeline pipeline = ch.pipeline();
        pipeline.addLast(l4LoadBalancer.connectionTracker());

        // HIGH-11: Track active connections for graceful draining
        if (activeConnections != null) {
            activeConnections.add(ch);
        }

        // Add PROXY protocol handler if enabled
        ProxyProtocolMode proxyMode = l4LoadBalancer.configurationContext().transportConfiguration().proxyProtocolMode();
        if (proxyMode != null && proxyMode != ProxyProtocolMode.OFF) {
            pipeline.addLast(new ProxyProtocolHandler(proxyMode));
        }

        // Add Connection Timeout Handler
        Duration timeout = Duration.ofMillis(l4LoadBalancer.configurationContext().transportConfiguration().connectionIdleTimeout());
        pipeline.addLast(new ConnectionTimeoutHandler(timeout, true));

        boolean available = l4LoadBalancer.configurationContext().tlsServerConfiguration().enabled();

        // Add SNI Handler if TLS is enabled
        if (available) {
            pipeline.addLast(new SNIHandler(l4LoadBalancer.configurationContext().tlsServerConfiguration()));
        }

        // Add Upstream Handler to handle Upstream Connections
        pipeline.addLast(new UpstreamHandler(l4LoadBalancer));

        // Log TLS availability
        log.debug("TLS for Server available: {}", available);
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
        log.error("Caught Error At ServerInitializer", cause);
    }
}
