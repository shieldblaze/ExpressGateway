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

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

class HealthCheckServiceTest {

    static Server server;
    static ManagedChannel channel;

    @BeforeAll
    static void setup() throws IOException {
        System.setProperty("EGWConfDir", System.getProperty("java.io.tmpdir"));

        server = NettyServerBuilder.forAddress(new InetSocketAddress("127.0.0.1", 60003))
                .addService(new HealthCheckService())
                .build()
                .start();

        channel = ManagedChannelBuilder.forTarget("127.0.0.1:60003")
                .usePlaintext()
                .build();
    }

    @AfterAll
    static void shutdown() {
        server.shutdown();
    }

    @Test
    void simpleTest() {
        HealthCheckServiceGrpc.HealthCheckServiceBlockingStub healthCheckService = HealthCheckServiceGrpc.newBlockingStub(channel);
        Configuration.HealthCheck healthCheck = Configuration.HealthCheck.newBuilder()
                .setWorkers(10)
                .setTimeInterval(100)
                .build();

        Configuration.ConfigurationResponse configurationResponse = healthCheckService.healthcheck(healthCheck);
        assertTrue(configurationResponse.getSuccess());
        assertEquals("Success", configurationResponse.getResponseText());
    }

    @Test
    void failingTest() {
        HealthCheckServiceGrpc.HealthCheckServiceBlockingStub healthCheckService = HealthCheckServiceGrpc.newBlockingStub(channel);
        Configuration.HealthCheck healthCheck = Configuration.HealthCheck.newBuilder()
                .setWorkers(-1)
                .setTimeInterval(100)
                .build();

        assertThrows(StatusRuntimeException.class, () -> healthCheckService.healthcheck(healthCheck));
    }
}
