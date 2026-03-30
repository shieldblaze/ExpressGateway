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

import com.shieldblaze.expressgateway.configuration.transport.TransportType;
import io.netty.channel.Channel;
import io.netty.channel.ChannelOption;
import io.netty.channel.epoll.EpollChannelOption;
import io.netty.channel.epoll.EpollSocketChannel;
import io.netty.channel.socket.SocketChannel;
import io.netty.incubator.channel.uring.IOUringChannelOption;
import io.netty.incubator.channel.uring.IOUringSocketChannel;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

/**
 * Transport-specific socket option tuner for TCP channels.
 *
 * <p>Applies kernel-level optimizations that vary by transport type:</p>
 * <ul>
 *   <li><strong>Epoll</strong>: TCP_QUICKACK for low-latency ACK, edge-triggered mode</li>
 *   <li><strong>io_uring</strong>: TCP_QUICKACK, TFO connect</li>
 *   <li><strong>NIO</strong>: Only standard Java socket options (TCP_NODELAY, SO_KEEPALIVE)</li>
 * </ul>
 *
 * <p>These tunings are applied to backend (downstream) channels after creation.
 * Frontend channels are tuned by TCPListener via ServerBootstrap options.</p>
 */
final class SocketTuner {

    private static final Logger logger = LogManager.getLogger(SocketTuner.class);

    private SocketTuner() {
    }

    /**
     * Socket tuning preset for different connection profiles.
     *
     * @param tcpNoDelay   disable Nagle's algorithm for latency-sensitive traffic
     * @param keepAlive    enable TCP keepalive for long-lived connections
     * @param quickAck     disable delayed ACK (native transports only)
     */
    record TuningProfile(boolean tcpNoDelay, boolean keepAlive, boolean quickAck) {

        /**
         * Low-latency profile: disable Nagle, disable delayed ACK, enable keepalive.
         * Suitable for proxied HTTP traffic, gRPC, and interactive protocols.
         */
        static final TuningProfile LOW_LATENCY = new TuningProfile(true, true, true);

        /**
         * Throughput profile: enable Nagle (batches small writes), no quick ACK, enable keepalive.
         * Suitable for bulk data transfer where latency is secondary to throughput.
         */
        static final TuningProfile THROUGHPUT = new TuningProfile(false, true, false);
    }

    /**
     * Apply transport-specific socket options to a backend channel.
     *
     * @param channel        the channel to tune
     * @param transportType  the transport type being used
     * @param profile        the tuning profile to apply
     */
    static void tune(Channel channel, TransportType transportType, TuningProfile profile) {
        if (!(channel instanceof SocketChannel)) {
            return;
        }

        try {
            channel.config().setOption(ChannelOption.TCP_NODELAY, profile.tcpNoDelay());
            channel.config().setOption(ChannelOption.SO_KEEPALIVE, profile.keepAlive());

            switch (transportType) {
                case EPOLL -> {
                    if (channel instanceof EpollSocketChannel epollChannel) {
                        if (profile.quickAck()) {
                            epollChannel.config().setOption(EpollChannelOption.TCP_QUICKACK, true);
                        }
                    }
                }
                case IO_URING -> {
                    if (channel instanceof IOUringSocketChannel ioUringChannel) {
                        if (profile.quickAck()) {
                            ioUringChannel.config().setOption(IOUringChannelOption.TCP_QUICKACK, true);
                        }
                    }
                }
                case NIO -> {
                    // NIO does not expose TCP_QUICKACK -- standard options only
                }
            }
        } catch (Exception e) {
            logger.debug("Failed to apply socket tuning for transport {}: {}",
                    transportType, e.getMessage());
        }
    }
}
