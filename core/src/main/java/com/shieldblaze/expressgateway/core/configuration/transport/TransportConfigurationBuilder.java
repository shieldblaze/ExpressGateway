package com.shieldblaze.expressgateway.core.configuration.transport;

import io.netty.util.internal.ObjectUtil;

public final class TransportConfigurationBuilder {
    private TransportType transportType;
    private ReceiveBufferAllocationType receiveBufferAllocationType;
    private int[] ReceiveBufferSizes;
    private int TCPConnectionBacklog;
    private int TCPDataBacklog;
    private int SocketReceiveBufferSize;
    private int SocketSendBufferSize;
    private int TCPFastOpenMaximumPendingRequestsCount;
    private int BackendConnectTimeout;

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

    public TransportConfigurationBuilder withTCPDataBacklog(int TCPDataBacklog) {
        this.TCPDataBacklog = TCPDataBacklog;
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

    public TransportConfigurationBuilder withBackendConnectTimeout(int BackendConnectTimeout) {
        this.BackendConnectTimeout = BackendConnectTimeout;
        return this;
    }

    public TransportConfiguration build() {
        TransportConfiguration transportConfiguration = new TransportConfiguration();
        transportConfiguration.setTransportType(ObjectUtil.checkNotNull(transportType, "Transport Type"));
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
        transportConfiguration.setTCPDataBacklog(ObjectUtil.checkPositive(TCPDataBacklog, "TCP Data Backlog"));

        transportConfiguration.setSocketReceiveBufferSize(SocketReceiveBufferSize);
        if (SocketReceiveBufferSize > 65536 || SocketReceiveBufferSize < 64) {
            throw new IllegalArgumentException("Socket Receive Buffer Size Cannot Be Less Than 64-65536");
        }

        transportConfiguration.setSocketSendBufferSize(SocketSendBufferSize);
        if (SocketSendBufferSize > 65536 || SocketSendBufferSize < 64) {
            throw new IllegalArgumentException("Socket Send Buffer Size Cannot Be Less Than 64-65536");
        }

        transportConfiguration.setTCPFastOpenMaximumPendingRequests(ObjectUtil.checkPositive(TCPFastOpenMaximumPendingRequestsCount,
                "TCP Fast Open Maximum Pending Requests"));
        transportConfiguration.setBackendConnectTimeout(ObjectUtil.checkPositive(BackendConnectTimeout, "Backend Connect Timeout"));

        return transportConfiguration;
    }
}
