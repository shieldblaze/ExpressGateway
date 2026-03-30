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
package com.shieldblaze.expressgateway.protocol.http.loadbalancer;

import io.netty.handler.codec.compression.BrotliMode;
import com.shieldblaze.expressgateway.configuration.ConfigurationContext;
import com.shieldblaze.expressgateway.configuration.http.HttpConfiguration;
import com.shieldblaze.expressgateway.core.L4FrontListener;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStopTask;
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancer;
import com.shieldblaze.expressgateway.metrics.MicrometerBridge;
import com.shieldblaze.expressgateway.protocol.http.Http11ServerInboundHandler;
import com.shieldblaze.expressgateway.protocol.http.Http2ServerInboundHandler;
import com.shieldblaze.expressgateway.protocol.http.HttpServerInitializer;
import io.micrometer.core.instrument.MeterRegistry;
import io.micrometer.core.instrument.simple.SimpleMeterRegistry;
import io.netty.channel.Channel;
import io.netty.channel.ChannelPipeline;
import io.netty.handler.codec.compression.CompressionOptions;
import io.netty.handler.codec.compression.StandardCompressionOptions;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.net.InetSocketAddress;

/**
 * HTTP Load Balancer
 */
public class HTTPLoadBalancer extends L4LoadBalancer {

    private static final Logger logger = LogManager.getLogger(HTTPLoadBalancer.class);

    private final CompressionOptions[] compressionOptions = new CompressionOptions[3];
    private final MeterRegistry meterRegistry;

    HTTPLoadBalancer(String name, InetSocketAddress bindAddress, L4FrontListener l4FrontListener,
                     ConfigurationContext configurationContext, HttpServerInitializer httpServerInitializer) {
        super(name, bindAddress, l4FrontListener, configurationContext, httpServerInitializer);
        httpServerInitializer.httpLoadBalancer(this);

        compressionOptions[0] = StandardCompressionOptions.brotli(httpConfiguration().brotliCompressionLevel(), 22, BrotliMode.GENERIC);
        compressionOptions[1] = StandardCompressionOptions.gzip(httpConfiguration().deflateCompressionLevel(), 15, 8);
        compressionOptions[2] = StandardCompressionOptions.deflate(httpConfiguration().deflateCompressionLevel(), 15, 8);

        // HI-01: Initialize Micrometer metrics bridge. Uses SimpleMeterRegistry by default.
        // For production Prometheus export, replace with PrometheusMeterRegistry and expose
        // the scrape endpoint. MicrometerBridge reads live values from the singleton recorder.
        meterRegistry = new SimpleMeterRegistry();
        MicrometerBridge.bind(meterRegistry);
    }

    /**
     * Get {@link HttpConfiguration} Instance for this {@link HTTPLoadBalancer}
     */
    public HttpConfiguration httpConfiguration() {
        return configurationContext().httpConfiguration();
    }

    public CompressionOptions[] compressionOptions() {
        return compressionOptions;
    }

    /**
     * HI-01: Get the Micrometer registry for this load balancer.
     * Can be replaced with a PrometheusMeterRegistry for Prometheus export.
     */
    public MeterRegistry meterRegistry() {
        return meterRegistry;
    }

    /**
     * HI-03: Coordinated graceful shutdown for HTTP/1.1 connections.
     *
     * <p>Before stopping the listener, initiate connection draining on all active
     * HTTP/1.1 connections: set the draining flag so new requests get 503 + Connection:close.
     * In-flight requests complete normally, then the connection closes.</p>
     *
     * <p>HTTP/2 connections are handled by the existing shutdown path — when the channel
     * closes (via cluster shutdown or EventLoop termination), the Http2ServerInboundHandler's
     * doClose() sends GOAWAY and schedules a delayed close per RFC 9113 Section 6.8.</p>
     */
    @Override
    public L4FrontListenerStopTask stop() {
        // Iterate all tracked channels and initiate H1 draining
        for (Channel ch : connectionTracker().allChannels()) {
            ch.eventLoop().execute(() -> {
                if (!ch.isActive()) {
                    return;
                }
                ChannelPipeline p = ch.pipeline();
                Http11ServerInboundHandler h1 = p.get(Http11ServerInboundHandler.class);
                if (h1 != null) {
                    h1.startDraining();
                }
            });
        }

        logger.info("HI-03: Initiated H1 graceful draining on {} active connections",
                connectionTracker().connections());

        return super.stop();
    }

    @Override
    public String type() {
        return "L7/HTTP";
    }
}
