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
package com.shieldblaze.expressgateway.core.configuration.transport;

import com.shieldblaze.expressgateway.core.configuration.CommonConfiguration;
import io.netty.channel.AdaptiveRecvByteBufAllocator;
import io.netty.channel.FixedRecvByteBufAllocator;
import io.netty.channel.RecvByteBufAllocator;

/**
 * Transport Configuration
 */
public final class TransportConfiguration extends CommonConfiguration {

    private TransportType transportType;
    private ReceiveBufferAllocationType receiveBufferAllocationType;
    private int[] ReceiveBufferSizes;
    private int TCPConnectionBacklog;
    private int DataBacklog;
    private int SocketReceiveBufferSize;
    private int SocketSendBufferSize;
    private int TCPFastOpenMaximumPendingRequests;
    private int BackendSocketTimeout;
    private int BackendConnectTimeout;
    private int ConnectionIdleTimeout;

    public TransportType getTransportType() {
        return transportType;
    }

    void setTransportType(TransportType transportType) {
        this.transportType = transportType;
    }

    public ReceiveBufferAllocationType getReceiveBufferAllocationType() {
        return receiveBufferAllocationType;
    }

    void setReceiveBufferAllocationType(ReceiveBufferAllocationType receiveBufferAllocationType) {
        this.receiveBufferAllocationType = receiveBufferAllocationType;
    }

    public int[] getReceiveBufferSizes() {
        return ReceiveBufferSizes;
    }

    void setReceiveBufferSizes(int[] receiveBufferSizes) {
        ReceiveBufferSizes = receiveBufferSizes;
    }

    public RecvByteBufAllocator getRecvByteBufAllocator() {
        if (receiveBufferAllocationType == ReceiveBufferAllocationType.FIXED) {
            return new FixedRecvByteBufAllocator(ReceiveBufferSizes[0]);
        } else {
            return new AdaptiveRecvByteBufAllocator(ReceiveBufferSizes[0], ReceiveBufferSizes[1], ReceiveBufferSizes[2]);
        }
    }

    public int getTCPConnectionBacklog() {
        return TCPConnectionBacklog;
    }

    void setTCPConnectionBacklog(int TCPConnectionBacklog) {
        this.TCPConnectionBacklog = TCPConnectionBacklog;
    }

    public int getDataBacklog() {
        return DataBacklog;
    }

    void setDataBacklog(int dataBacklog) {
        this.DataBacklog = dataBacklog;
    }

    public int getSocketReceiveBufferSize() {
        return SocketReceiveBufferSize;
    }

    void setSocketReceiveBufferSize(int socketReceiveBufferSize) {
        SocketReceiveBufferSize = socketReceiveBufferSize;
    }

    public int getSocketSendBufferSize() {
        return SocketSendBufferSize;
    }

    void setSocketSendBufferSize(int socketSendBufferSize) {
        SocketSendBufferSize = socketSendBufferSize;
    }

    public int getTCPFastOpenMaximumPendingRequests() {
        return TCPFastOpenMaximumPendingRequests;
    }

    void setTCPFastOpenMaximumPendingRequests(int TCPFastOpenMaximumPendingRequests) {
        this.TCPFastOpenMaximumPendingRequests = TCPFastOpenMaximumPendingRequests;
    }

    public int getBackendSocketTimeout() {
        return BackendSocketTimeout;
    }

    void setBackendSocketTimeout(int backendSocketTimeout) {
        BackendSocketTimeout = backendSocketTimeout;
    }

    public int getBackendConnectTimeout() {
        return BackendConnectTimeout;
    }

    void setBackendConnectTimeout(int backendConnectTimeout) {
        BackendConnectTimeout = backendConnectTimeout;
    }

    public int getConnectionIdleTimeout() {
        return ConnectionIdleTimeout;
    }

    void setConnectionIdleTimeout(int connectionIdleTimeout) {
        ConnectionIdleTimeout = connectionIdleTimeout;
    }
}
