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

import io.grpc.ManagedChannel;
import io.grpc.ManagedChannelBuilder;
import io.grpc.Server;
import io.grpc.StatusRuntimeException;
import io.grpc.netty.shaded.io.grpc.netty.NettyServerBuilder;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

import java.io.IOException;
import java.net.InetSocketAddress;
import java.util.Arrays;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

class TransportServiceTest {

    static Server server;
    static ManagedChannel channel;

    @BeforeAll
    static void setup() throws IOException {
        System.setProperty("EGWConfDir", System.getProperty("java.io.tmpdir"));

        server = NettyServerBuilder.forAddress(new InetSocketAddress("127.0.0.1", 60005))
                .addService(new TransportService())
                .build()
                .start();

        channel = ManagedChannelBuilder.forTarget("127.0.0.1:60005")
                .usePlaintext()
                .build();

    }

    @AfterAll
    static void shutdown() {
        channel.shutdown();
        server.shutdown();
    }

    @Test
    void simpleTest() {
        TransportServiceGrpc.TransportServiceBlockingStub transportService = TransportServiceGrpc.newBlockingStub(channel);
        Configuration.Transport transport = Configuration.Transport.newBuilder()
                .setConnectionIdleTimeout(1000)
                .setBackendConnectTimeout(1000)
                .setTcpFastOpenMaximumPendingRequests(1000)
                .setSocketSendBufferSize(1000)
                .setSocketReceiveBufferSize(1000)
                .setTcpConnectionBacklog(1000)
                .setReceiveBufferAllocationType(Configuration.Transport.ReceiveBufferAllocationType.FIXED)
                .setType(Configuration.Transport.Type.NIO)
                .addReceiveBufferSizes(1000)
                .build();

        Configuration.ConfigurationResponse configurationResponse = transportService.transport(transport);
        assertTrue(configurationResponse.getSuccess());
        assertEquals("Success", configurationResponse.getResponseText());
    }

    @Test
    void failingTest() {
        TransportServiceGrpc.TransportServiceBlockingStub transportService = TransportServiceGrpc.newBlockingStub(channel);
        Configuration.Transport transport = Configuration.Transport.newBuilder()
                .setConnectionIdleTimeout(1000)
                .setBackendConnectTimeout(1000)
                .setTcpFastOpenMaximumPendingRequests(1000)
                .setSocketSendBufferSize(1000)
                .setSocketReceiveBufferSize(1000)
                .setTcpConnectionBacklog(1000)
                .setReceiveBufferAllocationType(Configuration.Transport.ReceiveBufferAllocationType.FIXED)
                .setType(Configuration.Transport.Type.NIO)
                .addAllReceiveBufferSizes(Arrays.asList(1, 2)) // Invalid buffer sizes
                .build();

        assertThrows(StatusRuntimeException.class, () -> transportService.transport(transport));
    }
}
