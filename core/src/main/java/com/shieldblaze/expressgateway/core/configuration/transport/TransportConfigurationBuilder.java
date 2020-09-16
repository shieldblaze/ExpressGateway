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

import io.netty.channel.epoll.Epoll;
import io.netty.util.internal.ObjectUtil;

public final class TransportConfigurationBuilder {
    private TransportType transportType;
    private ReceiveBufferAllocationType receiveBufferAllocationType;
    private int[] ReceiveBufferSizes;
    private int TCPConnectionBacklog;
    private int DataBacklog;
    private int SocketReceiveBufferSize;
    private int SocketSendBufferSize;
    private int TCPFastOpenMaximumPendingRequestsCount;
    private int BackendSocketTimeout;
    private int BackendConnectTimeout;
    private int ConnectionIdleTimeout;

    private TransportConfigurationBuilder() {
    }

    public static TransportConfigurationBuilder newBuilder() {
        return new TransportConfigurationBuilder();
    }

    public TransportConfigurationBuilder withTransportType(TransportType transportType) {
        this.transportType = transportType;
        return this;
    }

    public TransportConfigurationBuilder withReceiveBufferAllocationType(ReceiveBufferAllocationType receiveBufferAllocationType) {
        this.receiveBufferAllocationType = receiveBufferAllocationType;
        return this;
    }

    public TransportConfigurationBuilder withReceiveBufferSizes(int[] ReceiveBufferSizes) {
        this.ReceiveBufferSizes = ReceiveBufferSizes;
        return this;
    }

    public TransportConfigurationBuilder withTCPConnectionBacklog(int TCPConnectionBacklog) {
        this.TCPConnectionBacklog = TCPConnectionBacklog;
        return this;
    }

    public TransportConfigurationBuilder withDataBacklog(int DataBacklog) {
        this.DataBacklog = DataBacklog;
        return this;
    }

    public TransportConfigurationBuilder withSocketReceiveBufferSize(int SocketReceiveBufferSize) {
        this.SocketReceiveBufferSize = SocketReceiveBufferSize;
        return this;
    }

    public TransportConfigurationBuilder withSocketSendBufferSize(int SocketSendBufferSize) {
        this.SocketSendBufferSize = SocketSendBufferSize;
        return this;
    }

    public TransportConfigurationBuilder withTCPFastOpenMaximumPendingRequests(int TCPFastOpenMaximumPendingRequests) {
        this.TCPFastOpenMaximumPendingRequestsCount = TCPFastOpenMaximumPendingRequests;
        return this;
    }

    public TransportConfigurationBuilder withBackendSocketTimeout(int BackendSocketTimeout) {
        this.BackendSocketTimeout = BackendSocketTimeout;
        return this;
    }

    public TransportConfigurationBuilder withBackendConnectTimeout(int BackendConnectTimeout) {
        this.BackendConnectTimeout = BackendConnectTimeout;
        return this;
    }

    public TransportConfigurationBuilder withConnectionIdleTimeout(int ConnectionIdleTimeout) {
        this.ConnectionIdleTimeout = ConnectionIdleTimeout;
        return this;
    }

    public TransportConfiguration build() {
        TransportConfiguration transportConfiguration = new TransportConfiguration();
        transportConfiguration.setTransportType(ObjectUtil.checkNotNull(transportType, "Transport Type"));

        if (transportType == TransportType.EPOLL && !Epoll.isAvailable()) {
            throw new IllegalArgumentException("Epoll is not available");
        }

        transportConfiguration.setReceiveBufferAllocationType(ObjectUtil.checkNotNull(receiveBufferAllocationType,
                "Receive Buffer Allocation Type"));
        transportConfiguration.setReceiveBufferSizes(ObjectUtil.checkNotNull(ReceiveBufferSizes, "Receive Buffer Sizes"));

        if (receiveBufferAllocationType == ReceiveBufferAllocationType.ADAPTIVE) {
            if (ReceiveBufferSizes.length != 3) {
                throw new IllegalArgumentException("Receive Buffer Sizes Are Invalid");
            }

            if (ReceiveBufferSizes[2] > 65536) {
                throw new IllegalArgumentException("Maximum Receive Buffer Size Cannot Be Greater Than 65536");
            } else if (ReceiveBufferSizes[2] < 64) {
                throw new IllegalArgumentException("Maximum Receive Buffer Size Cannot Be Less Than 64");
            }

            if (ReceiveBufferSizes[0] < 64 || ReceiveBufferSizes[0] > ReceiveBufferSizes[2]) {
                throw new IllegalArgumentException("Minimum Receive Buffer Size Must Be In Range Of 64-" + ReceiveBufferSizes[2]);
            }

            if (ReceiveBufferSizes[1] < 64 || ReceiveBufferSizes[1] > ReceiveBufferSizes[2] || ReceiveBufferSizes[1] < ReceiveBufferSizes[0]) {
                throw new IllegalArgumentException("Initial Receive Buffer Must Be In Range Of " + ReceiveBufferSizes[0] + "-" +
                        ReceiveBufferSizes[2]);
            }
        } else {
            if (ReceiveBufferSizes.length != 1) {
                throw new IllegalArgumentException("Receive Buffer Sizes Are Invalid");
            }

            if (ReceiveBufferSizes[0] > 65536 || ReceiveBufferSizes[0] < 64) {
                throw new IllegalArgumentException("Fixed Receive Buffer Size Cannot Be Less Than 64-65536");
            }
        }

        transportConfiguration.setTCPConnectionBacklog(ObjectUtil.checkPositive(TCPConnectionBacklog,  "TCP Connection Backlog"));
        transportConfiguration.setDataBacklog(ObjectUtil.checkPositive(DataBacklog, "Data Backlog"));

        transportConfiguration.setSocketReceiveBufferSize(SocketReceiveBufferSize);
        if (SocketReceiveBufferSize < 64) {
            throw new IllegalArgumentException("Socket Receive Buffer Size Must Be Greater Than 64");
        }

        transportConfiguration.setSocketSendBufferSize(SocketSendBufferSize);
        if (SocketSendBufferSize < 64) {
            throw new IllegalArgumentException("Socket Send Buffer Size Must Be Greater Than 64");
        }

        transportConfiguration.setTCPFastOpenMaximumPendingRequests(ObjectUtil.checkPositive(TCPFastOpenMaximumPendingRequestsCount,
                "TCP Fast Open Maximum Pending Requests"));
        transportConfiguration.setBackendConnectTimeout(ObjectUtil.checkPositive(BackendConnectTimeout, "Backend Connect Timeout"));
        transportConfiguration.setBackendSocketTimeout(ObjectUtil.checkPositive(BackendSocketTimeout, "Backend Socket Timeout"));
        transportConfiguration.setConnectionIdleTimeout(ObjectUtil.checkPositive(ConnectionIdleTimeout, "Connection Idle Timeout"));

        return transportConfiguration;
    }
}
