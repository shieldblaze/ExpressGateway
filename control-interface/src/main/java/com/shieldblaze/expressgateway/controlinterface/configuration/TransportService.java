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
package com.shieldblaze.expressgateway.controlinterface.configuration;

import com.shieldblaze.expressgateway.configuration.transport.ReceiveBufferAllocationType;
import com.shieldblaze.expressgateway.configuration.transport.TransportConfiguration;
import com.shieldblaze.expressgateway.configuration.transport.TransportConfigurationBuilder;
import com.shieldblaze.expressgateway.configuration.transport.TransportType;
import io.grpc.stub.StreamObserver;

public final class TransportService extends TransportServiceGrpc.TransportServiceImplBase {

    @Override
    public void transport(Configuration.Transport request, StreamObserver<Configuration.ConfigurationResponse> responseObserver) {
        Configuration.ConfigurationResponse response;

        try {
            Configuration.Transport.Type type = request.getType();
            TransportType transportType;

            if (type == Configuration.Transport.Type.NIO) {
                transportType = TransportType.NIO;
            } else if (type == Configuration.Transport.Type.EPOLL) {
                transportType = TransportType.EPOLL;
            } else if (type == Configuration.Transport.Type.IOURING) {
                transportType = TransportType.IO_URING;
            } else {
                throw new IllegalArgumentException("Unknown Transport Type: " + type);
            }

            Configuration.Transport.ReceiveBufferAllocationType rType = request.getReceiveBufferAllocationType();
            ReceiveBufferAllocationType receiveBufferAllocationType;

            if (rType == Configuration.Transport.ReceiveBufferAllocationType.FIXED) {
                receiveBufferAllocationType = ReceiveBufferAllocationType.FIXED;
            } else if (rType == Configuration.Transport.ReceiveBufferAllocationType.ADAPTIVE) {
                receiveBufferAllocationType = ReceiveBufferAllocationType.ADAPTIVE;
            } else {
                throw new IllegalArgumentException("Unknown Receive Allocator Type: " + rType);
            }

            int[] bufferSizes = new int[request.getReceiveBufferSizesList().size()];

            int count = 0;
            for (int i : request.getReceiveBufferSizesList()) {
                bufferSizes[count] = i;
                count++;
            }

            TransportConfiguration transportConfiguration = TransportConfigurationBuilder.newBuilder()
                    .withTCPFastOpenMaximumPendingRequests(request.getTcpFastOpenMaximumPendingRequests())
                    .withSocketReceiveBufferSize(request.getSocketReceiveBufferSize())
                    .withSocketSendBufferSize(request.getSocketSendBufferSize())
                    .withTCPConnectionBacklog(request.getTcpConnectionBacklog())
                    .withBackendConnectTimeout(request.getBackendConnectTimeout())
                    .withBackendSocketTimeout(request.getBackendSocketTimeout())
                    .withConnectionIdleTimeout(request.getConnectionIdleTimeout())
                    .withTransportType(transportType)
                    .withReceiveBufferSizes(bufferSizes)
                    .withReceiveBufferAllocationType(receiveBufferAllocationType)
                    .build();

            transportConfiguration.saveTo(request.getProfileName());

            response = Configuration.ConfigurationResponse.newBuilder()
                    .setResponseCode(1)
                    .setResponseText("Success")
                    .build();
        } catch (Exception ex) {
            response = Configuration.ConfigurationResponse.newBuilder()
                    .setResponseCode(-1)
                    .setResponseText("Error: " + ex.getLocalizedMessage())
                    .build();
        }

        responseObserver.onNext(response);
        responseObserver.onCompleted();
    }

    @Override
    public void get(Configuration.GetTransportService request, StreamObserver<Configuration.Transport> responseObserver) {
        try {
            TransportConfiguration transportConfiguration = TransportConfiguration.loadFrom(request.getProfileName());

            Configuration.Transport.Type type = Configuration.Transport.Type.NIO;
            if (transportConfiguration.transportType() == TransportType.EPOLL) {
                type = Configuration.Transport.Type.EPOLL;
            } else if (transportConfiguration.transportType() == TransportType.IO_URING) {
                type = Configuration.Transport.Type.IOURING;
            }

            Configuration.Transport.ReceiveBufferAllocationType receiveBufferAllocationType = Configuration.Transport.ReceiveBufferAllocationType.ADAPTIVE;
            if (transportConfiguration.receiveBufferAllocationType() == ReceiveBufferAllocationType.FIXED) {
                receiveBufferAllocationType = Configuration.Transport.ReceiveBufferAllocationType.FIXED;
            }

            Configuration.Transport transport = Configuration.Transport.newBuilder()
                    .setType(type)
                    .setReceiveBufferAllocationType(receiveBufferAllocationType)
                    .setTcpConnectionBacklog(transportConfiguration.tcpConnectionBacklog())
                    .setSocketReceiveBufferSize(transportConfiguration.socketReceiveBufferSize())
                    .setSocketSendBufferSize(transportConfiguration.socketSendBufferSize())
                    .setTcpFastOpenMaximumPendingRequests(transportConfiguration.tcpFastOpenMaximumPendingRequests())
                    .setBackendSocketTimeout(transportConfiguration.backendSocketTimeout())
                    .setBackendConnectTimeout(transportConfiguration.backendConnectTimeout())
                    .setConnectionIdleTimeout(transportConfiguration.connectionIdleTimeout())
                    .setProfileName(request.getProfileName())
                    .build();

            responseObserver.onNext(transport);
            responseObserver.onCompleted();
        } catch (Exception ex) {
            responseObserver.onError(ex);
            responseObserver.onCompleted();
        }
    }
}
