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
package com.shieldblaze.expressgateway.configuration.transport;

import io.netty.channel.epoll.Epoll;
import io.netty.incubator.channel.uring.IOUring;
import io.netty.util.internal.ObjectUtil;

import java.util.Objects;

/**
 * Configuration Builder for {@link TransportConfiguration}
 */
public final class TransportConfigurationBuilder {
    private TransportType transportType;
    private ReceiveBufferAllocationType receiveBufferAllocationType;
    private int[] receiveBufferSizes;
    private int tcpConnectionBacklog;
    private int socketReceiveBufferSize;
    private int socketSendBufferSize;
    private int tcpFastOpenMaximumPendingRequestsCount;
    private int backendSocketTimeout;
    private int backendConnectTimeout;
    private int connectionIdleTimeout;

    private TransportConfigurationBuilder() {
        // Prevent outside initialization
    }

    /**
     * Create a new {@link TransportConfigurationBuilder} Instance
     *
     * @return {@link TransportConfigurationBuilder} Instance
     */
    public static TransportConfigurationBuilder newBuilder() {
        return new TransportConfigurationBuilder();
    }

    /**
     * Set {@link TransportType} to use
     */
    public TransportConfigurationBuilder withTransportType(TransportType transportType) {
        this.transportType = transportType;
        return this;
    }

    /**
     * Set {@link ReceiveBufferAllocationType} to use
     */
    public TransportConfigurationBuilder withReceiveBufferAllocationType(ReceiveBufferAllocationType receiveBufferAllocationType) {
        this.receiveBufferAllocationType = receiveBufferAllocationType;
        return this;
    }

    /**
     * Set Receive Buffer Allocation Sizes
     */
    public TransportConfigurationBuilder withReceiveBufferSizes(int[] ReceiveBufferSizes) {
        this.receiveBufferSizes = ReceiveBufferSizes;
        return this;
    }

    /**
     * Set TCP Connection Backlog Size
     */
    public TransportConfigurationBuilder withTCPConnectionBacklog(int TCPConnectionBacklog) {
        this.tcpConnectionBacklog = TCPConnectionBacklog;
        return this;
    }

    /**
     * Set Socket Receive Buffer Size
     */
    public TransportConfigurationBuilder withSocketReceiveBufferSize(int SocketReceiveBufferSize) {
        this.socketReceiveBufferSize = SocketReceiveBufferSize;
        return this;
    }

    /**
     * Set Socket Send Buffer Size
     */
    public TransportConfigurationBuilder withSocketSendBufferSize(int SocketSendBufferSize) {
        this.socketSendBufferSize = SocketSendBufferSize;
        return this;
    }

    /**
     * Set TCP Fast Open Maximum Pending Requests Limit
     */
    public TransportConfigurationBuilder withTCPFastOpenMaximumPendingRequests(int TCPFastOpenMaximumPendingRequests) {
        this.tcpFastOpenMaximumPendingRequestsCount = TCPFastOpenMaximumPendingRequests;
        return this;
    }

    /**
     * Set Backend Socket Timeout
     */
    public TransportConfigurationBuilder withBackendSocketTimeout(int BackendSocketTimeout) {
        this.backendSocketTimeout = BackendSocketTimeout;
        return this;
    }

    /**
     * Set Backend Connect Timeout
     */
    public TransportConfigurationBuilder withBackendConnectTimeout(int BackendConnectTimeout) {
        this.backendConnectTimeout = BackendConnectTimeout;
        return this;
    }

    /**
     * Set Connection Idle Timeout
     */
    public TransportConfigurationBuilder withConnectionIdleTimeout(int ConnectionIdleTimeout) {
        this.connectionIdleTimeout = ConnectionIdleTimeout;
        return this;
    }

    /**
     * Build {@link TransportConfiguration}
     *
     * @return {@link TransportConfiguration} Instance
     * @throws NullPointerException     If a required value if {@code null}
     * @throws IllegalArgumentException If a value is invalid
     */
    public TransportConfiguration build() {

        Objects.requireNonNull(transportType, "Transport Type");
        Objects.requireNonNull(receiveBufferAllocationType, "Receive Buffer Allocation Type");
        Objects.requireNonNull(receiveBufferSizes, "Receive Buffer Sizes");
        ObjectUtil.checkPositive(tcpConnectionBacklog, "TCP Connection Backlog");
        ObjectUtil.checkPositive(tcpFastOpenMaximumPendingRequestsCount, "TCP Fast Open Maximum Pending Requests");
        ObjectUtil.checkPositive(backendConnectTimeout, "Backend Connect Timeout");
        ObjectUtil.checkPositive(backendSocketTimeout, "Backend Socket Timeout");
        ObjectUtil.checkPositive(connectionIdleTimeout, "Connection Idle Timeout");

        if (transportType == TransportType.EPOLL && !Epoll.isAvailable()) {
            throw new IllegalArgumentException("Epoll is not available");
        } else if (transportType == TransportType.IO_URING && !IOUring.isAvailable()) {
            throw new IllegalArgumentException("IOUring is not available");
        }

        if (receiveBufferAllocationType == ReceiveBufferAllocationType.ADAPTIVE) {
            if (receiveBufferSizes.length != 3) {
                throw new IllegalArgumentException("Receive Buffer Sizes Are Invalid");
            }

            if (receiveBufferSizes[2] > 65536) {
                throw new IllegalArgumentException("Maximum Receive Buffer Size Cannot Be Greater Than 65536");
            } else if (receiveBufferSizes[2] < 64) {
                throw new IllegalArgumentException("Maximum Receive Buffer Size Cannot Be Less Than 64");
            }

            if (receiveBufferSizes[0] < 64 || receiveBufferSizes[0] > receiveBufferSizes[2]) {
                throw new IllegalArgumentException("Minimum Receive Buffer Size Must Be In Range Of 64-" + receiveBufferSizes[2]);
            }

            if (receiveBufferSizes[1] < 64 || receiveBufferSizes[1] > receiveBufferSizes[2] || receiveBufferSizes[1] < receiveBufferSizes[0]) {
                throw new IllegalArgumentException("Initial Receive Buffer Must Be In Range Of " + receiveBufferSizes[0] + "-" + receiveBufferSizes[2]);
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

        return new TransportConfiguration()
                .transportType(transportType)
                .receiveBufferAllocationType(receiveBufferAllocationType)
                .receiveBufferSizes(receiveBufferSizes)
                .tcpConnectionBacklog(tcpConnectionBacklog)
                .socketReceiveBufferSize(socketReceiveBufferSize)
                .socketSendBufferSize(socketSendBufferSize)
                .tcpFastOpenMaximumPendingRequests(tcpFastOpenMaximumPendingRequestsCount)
                .backendConnectTimeout(backendConnectTimeout)
                .backendSocketTimeout(backendSocketTimeout)
                .connectionIdleTimeout(connectionIdleTimeout);
    }
}
