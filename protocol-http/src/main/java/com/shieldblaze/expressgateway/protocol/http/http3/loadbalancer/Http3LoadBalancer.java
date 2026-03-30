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
package com.shieldblaze.expressgateway.protocol.http.http3.loadbalancer;

import io.netty.handler.codec.compression.BrotliMode;
import com.shieldblaze.expressgateway.configuration.ConfigurationContext;
import com.shieldblaze.expressgateway.configuration.http3.Http3Configuration;
import com.shieldblaze.expressgateway.configuration.quic.QuicConfiguration;
import com.shieldblaze.expressgateway.core.L4FrontListener;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStopTask;
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancer;
import com.shieldblaze.expressgateway.metrics.MicrometerBridge;
import com.shieldblaze.expressgateway.protocol.http.http3.Http3Bootstrapper;
import com.shieldblaze.expressgateway.protocol.http.http3.Http3Connection;
import com.shieldblaze.expressgateway.protocol.quic.QuicConnectionPool;
import io.micrometer.core.instrument.MeterRegistry;
import io.micrometer.core.instrument.simple.SimpleMeterRegistry;
import com.shieldblaze.expressgateway.protocol.http.http3.Http3Constants;
import io.netty.buffer.Unpooled;
import io.netty.channel.Channel;
import io.netty.handler.codec.compression.CompressionOptions;
import io.netty.handler.codec.compression.StandardCompressionOptions;
import io.netty.handler.codec.quic.QuicChannel;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.net.InetSocketAddress;
import java.util.concurrent.TimeUnit;

/**
 * HTTP/3 Load Balancer backed by QUIC transport (RFC 9000) and HTTP/3 (RFC 9114).
 *
 * <p>This load balancer manages frontend QUIC connections, applies HTTP/3 framing,
 * and proxies requests to backend Nodes. Backend connections are managed through
 * {@link QuicConnectionPool} (QUIC connection pooling with stream multiplexing)
 * and created via {@link Http3Bootstrapper}.</p>
 */
public class Http3LoadBalancer extends L4LoadBalancer {

    private static final Logger logger = LogManager.getLogger(Http3LoadBalancer.class);

    private final CompressionOptions[] compressionOptions = new CompressionOptions[3];
    private final MeterRegistry meterRegistry;
    private final QuicConnectionPool<Http3Connection> connectionPool;
    private final Http3Bootstrapper bootstrapper;

    Http3LoadBalancer(String name, InetSocketAddress bindAddress, L4FrontListener l4FrontListener,
                      ConfigurationContext configurationContext) {
        super(name, bindAddress, l4FrontListener, configurationContext, null);

        QuicConfiguration quicConfig = quicConfiguration();

        compressionOptions[0] = StandardCompressionOptions.brotli(
                configurationContext.httpConfiguration().brotliCompressionLevel(), 22, BrotliMode.GENERIC);
        compressionOptions[1] = StandardCompressionOptions.gzip(
                configurationContext.httpConfiguration().deflateCompressionLevel(), 15, 8);
        compressionOptions[2] = StandardCompressionOptions.deflate(
                configurationContext.httpConfiguration().deflateCompressionLevel(), 15, 8);

        this.connectionPool = new QuicConnectionPool<>(quicConfig);
        this.bootstrapper = new Http3Bootstrapper(
                configurationContext,
                eventLoopFactory().childGroup(),
                byteBufAllocator()
        );

        meterRegistry = new SimpleMeterRegistry();
        MicrometerBridge.bind(meterRegistry);
    }

    /**
     * Get {@link QuicConfiguration} for this load balancer.
     */
    public QuicConfiguration quicConfiguration() {
        return configurationContext().quicConfiguration();
    }

    /**
     * Get {@link Http3Configuration} for this load balancer.
     */
    public Http3Configuration http3Configuration() {
        return configurationContext().http3Configuration();
    }

    /**
     * Get the QUIC backend connection pool.
     */
    public QuicConnectionPool<Http3Connection> connectionPool() {
        return connectionPool;
    }

    /**
     * Get the HTTP/3 backend connection bootstrapper.
     */
    public Http3Bootstrapper bootstrapper() {
        return bootstrapper;
    }

    public CompressionOptions[] compressionOptions() {
        return compressionOptions;
    }

    public MeterRegistry meterRegistry() {
        return meterRegistry;
    }

    /**
     * Graceful shutdown for HTTP/3 connections per RFC 9114 Section 5.2.
     */
    @Override
    public L4FrontListenerStopTask stop() {
        long drainMs = quicConfiguration().gracefulShutdownDrainMs();

        for (Channel ch : connectionTracker().allChannels()) {
            ch.eventLoop().execute(() -> {
                if (!ch.isActive()) {
                    return;
                }

                if (ch instanceof QuicChannel quicChannel) {
                    // RFC 9114 Section 5.2: Mark the connection as draining so new request
                    // streams are rejected with 503 at the Http3ServerHandler level.
                    quicChannel.attr(Http3Constants.DRAINING_KEY).set(Boolean.TRUE);

                    // RFC 9114 Section 5.2: H3_NO_ERROR (0x100) for HTTP/3 graceful shutdown
                    quicChannel.close(true, 0x100, Unpooled.EMPTY_BUFFER)
                            .addListener(future -> {
                                if (!future.isSuccess()) {
                                    logger.warn("Failed to send GOAWAY on QUIC connection: {}",
                                            future.cause() != null ? future.cause().getMessage() : "unknown");
                                }
                            });

                    ch.eventLoop().schedule(() -> {
                        if (ch.isActive()) {
                            ch.close();
                        }
                    }, drainMs, TimeUnit.MILLISECONDS);
                }
            });
        }

        logger.info("Initiated HTTP/3 graceful GOAWAY draining on {} active connections, drain timeout {}ms",
                connectionTracker().connections(), drainMs);

        // Close backend connection pool AFTER drain period so in-flight responses
        // can complete. Without this delay, responses in progress are dropped.
        eventLoopFactory().parentGroup().schedule(
                () -> connectionPool.closeAll(),
                drainMs + 1000, TimeUnit.MILLISECONDS);

        return super.stop();
    }

    @Override
    public String type() {
        return "L7/HTTP3";
    }
}
