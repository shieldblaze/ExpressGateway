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

import io.netty.channel.AdaptiveRecvByteBufAllocator;
import io.netty.channel.FixedRecvByteBufAllocator;
import io.netty.channel.RecvByteBufAllocator;
import io.netty.channel.epoll.Epoll;
import io.netty.incubator.channel.uring.IOUring;

/**
 * Transport Configuration
 */
public final class TransportConfiguration {

    private TransportType transportType;
    private ReceiveBufferAllocationType receiveBufferAllocationType;
    private int[] receiveBufferSizes;
    private int tcpConnectionBacklog;
    private int socketReceiveBufferSize;
    private int socketSendBufferSize;
    private int tcpFastOpenMaximumPendingRequests;
    private int backendConnectTimeout;
    private int connectionIdleTimeout;

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
        DEFAULT.receiveBufferSizes(new int[]{512, 9001, 65535});
        DEFAULT.tcpConnectionBacklog = 50_000;
        DEFAULT.socketSendBufferSize = 67108864;
        DEFAULT.socketReceiveBufferSize = 67108864;
        DEFAULT.tcpFastOpenMaximumPendingRequests = 100_000;
        DEFAULT.backendConnectTimeout = 1000 * 10;  // 10 Seconds
        DEFAULT.connectionIdleTimeout = 1000 * 10; // 2 Minute
    }

    public TransportType transportType() {
        return transportType;
    }

    TransportConfiguration transportType(TransportType transportType) {
        this.transportType = transportType;
        return this;
    }

    public ReceiveBufferAllocationType receiveBufferAllocationType() {
        return receiveBufferAllocationType;
    }

    TransportConfiguration receiveBufferAllocationType(ReceiveBufferAllocationType receiveBufferAllocationType) {
        this.receiveBufferAllocationType = receiveBufferAllocationType;
        return this;
    }

    public int[] receiveBufferSizes() {
        return receiveBufferSizes;
    }

    TransportConfiguration receiveBufferSizes(int[] receiveBufferSizes) {
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

    TransportConfiguration tcpConnectionBacklog(int TCPConnectionBacklog) {
        this.tcpConnectionBacklog = TCPConnectionBacklog;
        return this;
    }

    public int socketReceiveBufferSize() {
        return socketReceiveBufferSize;
    }

    TransportConfiguration socketReceiveBufferSize(int socketReceiveBufferSize) {
        this.socketReceiveBufferSize = socketReceiveBufferSize;
        return this;
    }

    public int socketSendBufferSize() {
        return socketSendBufferSize;
    }

    TransportConfiguration socketSendBufferSize(int socketSendBufferSize) {
        this.socketSendBufferSize = socketSendBufferSize;
        return this;
    }

    public int tcpFastOpenMaximumPendingRequests() {
        return tcpFastOpenMaximumPendingRequests;
    }

    TransportConfiguration tcpFastOpenMaximumPendingRequests(int TCPFastOpenMaximumPendingRequests) {
        this.tcpFastOpenMaximumPendingRequests = TCPFastOpenMaximumPendingRequests;
        return this;
    }

    public int backendConnectTimeout() {
        return backendConnectTimeout;
    }

    TransportConfiguration backendConnectTimeout(int backendConnectTimeout) {
        this.backendConnectTimeout = backendConnectTimeout;
        return this;
    }

    public int connectionIdleTimeout() {
        return connectionIdleTimeout;
    }

    TransportConfiguration connectionIdleTimeout(int connectionIdleTimeout) {
        this.connectionIdleTimeout = connectionIdleTimeout;
        return this;
    }
}
