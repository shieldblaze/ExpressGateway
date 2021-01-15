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
import io.grpc.ServerBuilder;
import io.grpc.netty.shaded.io.grpc.netty.NettyServerBuilder;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

import java.io.IOException;
import java.net.InetSocketAddress;

import static org.junit.jupiter.api.Assertions.*;

class HealthCheckServiceTest {

    static Server server;

    @BeforeAll
    static void setup() throws IOException {
        System.setProperty("EGWConfDir", System.getProperty("java.io.tmpdir"));

        server = NettyServerBuilder.forAddress(new InetSocketAddress("127.0.0.1", 9110))
                .addService(new HealthCheckService())
                .build()
                .start();
    }

    @AfterAll
    static void shutdown() {
        server.shutdownNow();
    }

    @Test
    void simpleTest() {
        ManagedChannel channel = ManagedChannelBuilder.forTarget("127.0.0.1:9110")
                .usePlaintext()
                .build();

        HealthCheckServiceGrpc.HealthCheckServiceBlockingStub healthCheckService = HealthCheckServiceGrpc.newBlockingStub(channel);
        Configuration.HealthCheck healthCheck = Configuration.HealthCheck.newBuilder()
                .setProfileName("Meow")
                .setWorkers(10)
                .setTimeInterval(100)
                .build();

        Configuration.ConfigurationResponse configurationResponse = healthCheckService.healthcheck(healthCheck);
        assertEquals(1, configurationResponse.getResponseCode());
        assertEquals("Success", configurationResponse.getResponseText());

        channel.shutdownNow();
    }

    @Test
    void failingTest() {
        ManagedChannel channel = ManagedChannelBuilder.forTarget("127.0.0.1:9110")
                .usePlaintext()
                .build();

        HealthCheckServiceGrpc.HealthCheckServiceBlockingStub healthCheckService = HealthCheckServiceGrpc.newBlockingStub(channel);
        Configuration.HealthCheck healthCheck = Configuration.HealthCheck.newBuilder()
                .setProfileName("Meow")
                .setWorkers(-1)
                .setTimeInterval(100)
                .build();

        Configuration.ConfigurationResponse configurationResponse = healthCheckService.healthcheck(healthCheck);
        assertEquals(-1, configurationResponse.getResponseCode());

        channel.shutdownNow();
    }
}
