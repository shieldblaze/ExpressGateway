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
package com.shieldblaze.expressgateway.configuration.transport;

import com.fasterxml.jackson.annotation.JsonProperty;
import com.shieldblaze.expressgateway.configuration.Configuration;
import io.netty.channel.AdaptiveRecvByteBufAllocator;
import io.netty.channel.FixedRecvByteBufAllocator;
import io.netty.channel.RecvByteBufAllocator;
import io.netty.channel.epoll.Epoll;
import io.netty.incubator.channel.uring.IOUring;
import io.netty.util.internal.ObjectUtil;

import lombok.Getter;
import lombok.Setter;
import lombok.ToString;
import lombok.experimental.Accessors;

import java.util.Objects;

/**
 * Transport Configuration
 */
@Getter
@Setter
@Accessors(fluent = true, chain = true)
@ToString
public final class TransportConfiguration implements Configuration<TransportConfiguration> {

    @JsonProperty("transportType")
    private TransportType transportType;

    @JsonProperty("receiveBufferAllocationType")
    private ReceiveBufferAllocationType receiveBufferAllocationType;

    @JsonProperty("receiveBufferSizes")
    private int[] receiveBufferSizes;

    @JsonProperty("tcpConnectionBacklog")
    private int tcpConnectionBacklog;

    @JsonProperty("socketReceiveBufferSize")
    private int socketReceiveBufferSize;

    @JsonProperty("socketSendBufferSize")
    private int socketSendBufferSize;

    @JsonProperty("tcpFastOpenMaximumPendingRequests")
    private int tcpFastOpenMaximumPendingRequests;

    @JsonProperty("backendConnectTimeout")
    private int backendConnectTimeout;

    @JsonProperty("connectionIdleTimeout")
    private int connectionIdleTimeout;

    @JsonProperty("proxyProtocolMode")
    private ProxyProtocolMode proxyProtocolMode;

    @JsonProperty("backendProxyProtocolMode")
    private BackendProxyProtocolMode backendProxyProtocolMode;

    public static final TransportConfiguration DEFAULT = new TransportConfiguration();

    static {
        if (IOUring.isAvailable()) {
            DEFAULT.transportType = TransportType.IO_URING;
        } else if (Epoll.isAvailable()) {
            DEFAULT.transportType = TransportType.EPOLL;
        } else {
            DEFAULT.transportType = TransportType.NIO;
        }

        DEFAULT.receiveBufferAllocationType = ReceiveBufferAllocationType.ADAPTIVE;
        DEFAULT.receiveBufferSizes = new int[]{512, 9001, 65535};
        DEFAULT.tcpConnectionBacklog = 50_000;
        DEFAULT.socketSendBufferSize = 262_144;     // 256 KB — sane default; 67 MB per socket exhausts memory at scale
        DEFAULT.socketReceiveBufferSize = 262_144;  // 256 KB — kernel auto-tunes beyond this when needed
        DEFAULT.tcpFastOpenMaximumPendingRequests = 100_000;
        DEFAULT.backendConnectTimeout = 1000 * 10;  // 10 Seconds
        DEFAULT.connectionIdleTimeout = 1000 * 120; // 2 Minute
        DEFAULT.proxyProtocolMode = ProxyProtocolMode.OFF;
        DEFAULT.backendProxyProtocolMode = BackendProxyProtocolMode.OFF;
    }

    /**
     * Returns a new appropriate {@link RecvByteBufAllocator} implementation
     */
    public RecvByteBufAllocator recvByteBufAllocator() {
        if (receiveBufferAllocationType == ReceiveBufferAllocationType.FIXED) {
            return new FixedRecvByteBufAllocator(receiveBufferSizes[0]);
        } else {
            return new AdaptiveRecvByteBufAllocator(receiveBufferSizes[0], receiveBufferSizes[1], receiveBufferSizes[2]);
        }
    }

    public ProxyProtocolMode proxyProtocolMode() {
        if (proxyProtocolMode == null) {
            return ProxyProtocolMode.OFF;
        }
        return proxyProtocolMode;
    }

    public BackendProxyProtocolMode backendProxyProtocolMode() {
        if (backendProxyProtocolMode == null) {
            return BackendProxyProtocolMode.OFF;
        }
        return backendProxyProtocolMode;
    }

    /**
     * Validate all parameters of this configuration
     *
     * @return this class instance
     * @throws IllegalArgumentException If any value is invalid
     * @throws NullPointerException     If any value is null
     */
    @Override
    public TransportConfiguration validate() {
        Objects.requireNonNull(transportType, "Transport Type");
        Objects.requireNonNull(receiveBufferAllocationType, "Receive Buffer Allocation Type");
        Objects.requireNonNull(receiveBufferSizes, "Receive Buffer Sizes");
        ObjectUtil.checkPositive(tcpConnectionBacklog, "TCP Connection Backlog");
        ObjectUtil.checkPositive(tcpFastOpenMaximumPendingRequests, "TCP Fast Open Maximum Pending Requests");
        ObjectUtil.checkPositive(backendConnectTimeout, "Backend Connect Timeout");
        ObjectUtil.checkPositive(connectionIdleTimeout, "Connection Idle Timeout");

        if (transportType == TransportType.EPOLL && !Epoll.isAvailable()) {
            throw new IllegalArgumentException("Epoll is not available");
        }
        if (transportType == TransportType.IO_URING && !IOUring.isAvailable()) {
            throw new IllegalArgumentException("IOUring is not available");
        }

        if (receiveBufferAllocationType == ReceiveBufferAllocationType.ADAPTIVE) {
            if (receiveBufferSizes.length != 3) {
                throw new IllegalArgumentException("Receive Buffer Sizes Are Invalid");
            }

            if (receiveBufferSizes[2] > 65535) {
                throw new IllegalArgumentException("Maximum Receive Buffer Size Cannot Be Greater Than 65535");
            }
            if (receiveBufferSizes[2] < 64) {
                throw new IllegalArgumentException("Maximum Receive Buffer Size Cannot Be Less Than 64");
            }

            if (receiveBufferSizes[0] < 64 || receiveBufferSizes[0] > receiveBufferSizes[2]) {
                throw new IllegalArgumentException("Minimum Receive Buffer Size Must Be In Range Of 64-" + receiveBufferSizes[2]);
            }

            if (receiveBufferSizes[1] < 64 || receiveBufferSizes[1] > receiveBufferSizes[2] || receiveBufferSizes[1] < receiveBufferSizes[0]) {
                throw new IllegalArgumentException("Initial Receive Buffer Must Be In Range Of " + receiveBufferSizes[0] + '-' + receiveBufferSizes[2]);
            }
        } else {
            if (receiveBufferSizes.length != 1) {
                throw new IllegalArgumentException("Receive Buffer Sizes Are Invalid");
            }

            if (receiveBufferSizes[0] > 65536 || receiveBufferSizes[0] < 64) {
                throw new IllegalArgumentException("Fixed Receive Buffer Size Cannot Be Less Than 64-65536");
            }
        }

        if (socketReceiveBufferSize < 64) {
            throw new IllegalArgumentException("Socket Receive Buffer Size Must Be Greater Than 64");
        }

        if (socketSendBufferSize < 64) {
            throw new IllegalArgumentException("Socket Send Buffer Size Must Be Greater Than 64");
        }

        return this;
    }
}
