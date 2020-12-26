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
        return new TransportConfiguration()
                .transportType(transportType)
                .receiveBufferAllocationType(receiveBufferAllocationType)
                .receiveBufferSizes(receiveBufferSizes)
                .tcpConnectionBacklog(tcpConnectionBacklog)
                .socketReceiveBufferSize(socketReceiveBufferSize)
                .socketSendBufferSize(socketSendBufferSize)
                .tcpFastOpenMaximumPendingRequests(tcpFastOpenMaximumPendingRequestsCount)
                .backendConnectTimeout(backendConnectTimeout)
                .connectionIdleTimeout(connectionIdleTimeout);
    }
}
