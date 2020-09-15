package com.shieldblaze.expressgateway.core.configuration.transport;

public class TransportConfiguration {
    private TransportType transportType;
    private ReceiveBufferAllocationType receiveBufferAllocationType;
    private int[] ReceiveBufferSizes;
    private int TCPConnectionBacklog;
    private int TCPDataBacklog;
    private int SocketReceiveBufferSize;
    private int SocketSendBufferSize;
    private int TCPFastOpenMaximumPendingRequests;
    private int BackendConnectTimeout;

    public TransportType getTransportType() {
        return transportType;
    }

    public void setTransportType(TransportType transportType) {
        this.transportType = transportType;
    }

    public ReceiveBufferAllocationType getReceiveBufferAllocationType() {
        return receiveBufferAllocationType;
    }

    public void setReceiveBufferAllocationType(ReceiveBufferAllocationType receiveBufferAllocationType) {
        this.receiveBufferAllocationType = receiveBufferAllocationType;
    }

    public int[] getReceiveBufferSizes() {
        return ReceiveBufferSizes;
    }

    public void setReceiveBufferSizes(int[] receiveBufferSizes) {
        ReceiveBufferSizes = receiveBufferSizes;
    }

    public int getTCPConnectionBacklog() {
        return TCPConnectionBacklog;
    }

    public void setTCPConnectionBacklog(int TCPConnectionBacklog) {
        this.TCPConnectionBacklog = TCPConnectionBacklog;
    }

    public int getTCPDataBacklog() {
        return TCPDataBacklog;
    }

    public void setTCPDataBacklog(int TCPDataBacklog) {
        this.TCPDataBacklog = TCPDataBacklog;
    }

    public int getSocketReceiveBufferSize() {
        return SocketReceiveBufferSize;
    }

    public void setSocketReceiveBufferSize(int socketReceiveBufferSize) {
        SocketReceiveBufferSize = socketReceiveBufferSize;
    }

    public int getSocketSendBufferSize() {
        return SocketSendBufferSize;
    }

    public void setSocketSendBufferSize(int socketSendBufferSize) {
        SocketSendBufferSize = socketSendBufferSize;
    }

    public int getTCPFastOpenMaximumPendingRequests() {
        return TCPFastOpenMaximumPendingRequests;
    }

    public void setTCPFastOpenMaximumPendingRequests(int TCPFastOpenMaximumPendingRequests) {
        this.TCPFastOpenMaximumPendingRequests = TCPFastOpenMaximumPendingRequests;
    }

    public int getBackendConnectTimeout() {
        return BackendConnectTimeout;
    }

    public void setBackendConnectTimeout(int backendConnectTimeout) {
        BackendConnectTimeout = backendConnectTimeout;
    }
}
