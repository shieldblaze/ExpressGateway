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
package com.shieldblaze.expressgateway.configuration.transport;

import com.google.gson.annotations.Expose;
import com.shieldblaze.expressgateway.common.utils.Number;
import com.shieldblaze.expressgateway.configuration.Configuration;
import io.netty.channel.AdaptiveRecvByteBufAllocator;
import io.netty.channel.FixedRecvByteBufAllocator;
import io.netty.channel.RecvByteBufAllocator;
import io.netty.channel.epoll.Epoll;
import io.netty.incubator.channel.uring.IOUring;

import java.util.Arrays;
import java.util.Objects;

/**
 * Transport Configuration
 */
public final class TransportConfiguration implements Configuration {

    public static final TransportConfiguration EMPTY_INSTANCE = new TransportConfiguration();

    @Expose
    private TransportType transportType;

    @Expose
    private ReceiveBufferAllocationType receiveBufferAllocationType;

    @Expose
    private int[] receiveBufferSizes;

    @Expose
    private int tcpConnectionBacklog;

    @Expose
    private int socketReceiveBufferSize;

    @Expose
    private int socketSendBufferSize;

    @Expose
    private int tcpFastOpenMaximumPendingRequests;

    @Expose
    private int backendConnectTimeout;

    @Expose
    private int connectionIdleTimeout;

    public TransportType transportType() {
        return transportType;
    }

    public TransportConfiguration transportType(TransportType transportType) {
        this.transportType = transportType;
        return this;
    }

    public ReceiveBufferAllocationType receiveBufferAllocationType() {
        return receiveBufferAllocationType;
    }

    public TransportConfiguration receiveBufferAllocationType(ReceiveBufferAllocationType receiveBufferAllocationType) {
        this.receiveBufferAllocationType = Objects.requireNonNull(receiveBufferAllocationType);
        return this;
    }

    public int[] receiveBufferSizes() {
        return receiveBufferSizes;
    }

    public TransportConfiguration receiveBufferSizes(int[] receiveBufferSizes) {
        this.receiveBufferSizes = receiveBufferSizes;
        return this;
    }

    public RecvByteBufAllocator recvByteBufAllocator() {
        if (receiveBufferAllocationType == ReceiveBufferAllocationType.FIXED) {
            return new FixedRecvByteBufAllocator(receiveBufferSizes[0]);
        } else {
            return new AdaptiveRecvByteBufAllocator(receiveBufferSizes[0], receiveBufferSizes[1], receiveBufferSizes[2]);
        }
    }

    public int tcpConnectionBacklog() {
        return tcpConnectionBacklog;
    }

    public TransportConfiguration tcpConnectionBacklog(int tcpConnectionBacklog) {
        this.tcpConnectionBacklog = Number.checkPositive(tcpConnectionBacklog, "TCP Connection Backlog");
        return this;
    }

    public int socketReceiveBufferSize() {
        return socketReceiveBufferSize;
    }

    public TransportConfiguration socketReceiveBufferSize(int socketReceiveBufferSize) {
        if (socketReceiveBufferSize < 64) {
            throw new IllegalArgumentException("Socket Receive Buffer Size Must Be Greater Than 64");
        }
        this.socketReceiveBufferSize = socketReceiveBufferSize;
        return this;
    }

    public int socketSendBufferSize() {
        return socketSendBufferSize;
    }

    public TransportConfiguration socketSendBufferSize(int socketSendBufferSize) {
        if (socketSendBufferSize < 64) {
            throw new IllegalArgumentException("Socket Send Buffer Size Must Be Greater Than 64");
        }
        this.socketSendBufferSize = socketSendBufferSize;
        return this;
    }

    public int tcpFastOpenMaximumPendingRequests() {
        return tcpFastOpenMaximumPendingRequests;
    }

    public TransportConfiguration tcpFastOpenMaximumPendingRequests(int tcpFastOpenMaximumPendingRequests) {
        this.tcpFastOpenMaximumPendingRequests = Number.checkPositive(tcpFastOpenMaximumPendingRequests, "TCP Fast Open Maximum Pending Requests");
        return this;
    }

    public int backendConnectTimeout() {
        return backendConnectTimeout;
    }

    public TransportConfiguration backendConnectTimeout(int backendConnectTimeout) {
        this.backendConnectTimeout = Number.checkPositive(backendConnectTimeout, "Backend Connect Timeout");
        return this;
    }

    public int connectionIdleTimeout() {
        return connectionIdleTimeout;
    }

    public TransportConfiguration connectionIdleTimeout(int connectionIdleTimeout) {
        this.connectionIdleTimeout = Number.checkPositive(connectionIdleTimeout, "Connection Idle Timeout");
        return this;
    }

    @Override
    public String name() {
        return "Transport";
    }

    @Override
    public void validate() throws Exception {
        Objects.requireNonNull(transportType);
        if (transportType == TransportType.EPOLL && !Epoll.isAvailable()) {
            throw new IllegalArgumentException("Epoll is not available");
        } else if (transportType == TransportType.IO_URING && !IOUring.isAvailable()) {
            throw new IllegalArgumentException("IOUring is not available");
        }

        Objects.requireNonNull(receiveBufferAllocationType);

        Objects.requireNonNull(receiveBufferSizes, "Receive Buffer Sizes");

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

        Number.checkPositive(tcpConnectionBacklog, "TCP Connection Backlog");
        if (socketReceiveBufferSize < 64) {
            throw new IllegalArgumentException("Socket Receive Buffer Size Must Be Greater Than 64");
        }
        if (socketSendBufferSize < 64) {
            throw new IllegalArgumentException("Socket Send Buffer Size Must Be Greater Than 64");
        }
        Number.checkPositive(tcpFastOpenMaximumPendingRequests, "TCP Fast Open Maximum Pending Requests");
        Number.checkPositive(backendConnectTimeout, "Backend Connect Timeout");
        Number.checkPositive(connectionIdleTimeout, "Connection Idle Timeout");
    }
}
