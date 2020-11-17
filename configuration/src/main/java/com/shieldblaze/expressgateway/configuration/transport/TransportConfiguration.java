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

import com.shieldblaze.expressgateway.configuration.CoreConfiguration;
import io.netty.channel.AdaptiveRecvByteBufAllocator;
import io.netty.channel.FixedRecvByteBufAllocator;
import io.netty.channel.RecvByteBufAllocator;

/**
 * Transport Configuration
 */
public final class TransportConfiguration extends CoreConfiguration {

    private TransportType transportType;
    private ReceiveBufferAllocationType receiveBufferAllocationType;
    private int[] receiveBufferSizes;
    private int tcpConnectionBacklog;
    private int dataBacklog;
    private int socketReceiveBufferSize;
    private int socketSendBufferSize;
    private int tcpFastOpenMaximumPendingRequests;
    private int backendSocketTimeout;
    private int backendConnectTimeout;
    private int connectionIdleTimeout;

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

    public int dataBacklog() {
        return dataBacklog;
    }

    TransportConfiguration dataBacklog(int dataBacklog) {
        this.dataBacklog = dataBacklog;
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

    public int backendSocketTimeout() {
        return backendSocketTimeout;
    }

    TransportConfiguration backendSocketTimeout(int backendSocketTimeout) {
        this.backendSocketTimeout = backendSocketTimeout;
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
