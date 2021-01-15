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
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

import java.io.IOException;

import static org.junit.jupiter.api.Assertions.*;

class EventLoopServiceTest {

    static Server server;

    @BeforeAll
    static void setup() throws IOException {
        System.setProperty("EGWConfDir", System.getProperty("java.io.tmpdir"));

        server = ServerBuilder.forPort(2000)
                .addService(new EventLoopService())
                .build()
                .start();
    }

    @AfterAll
    static void shutdown() {
        server.shutdownNow();
    }

    @Test
    void simpleTest() {
        ManagedChannel channel = ManagedChannelBuilder.forTarget("127.0.0.1:2000")
                .usePlaintext()
                .build();

        EventLoopServiceGrpc.EventLoopServiceBlockingStub eventLoopService = EventLoopServiceGrpc.newBlockingStub(channel);
        Configuration.EventLoop eventLoop = Configuration.EventLoop.newBuilder()
                .setProfileName("Meow")
                .setParentWorkers(2)
                .setChildWorkers(4)
                .build();

        Configuration.ConfigurationResponse configurationResponse = eventLoopService.eventLoop(eventLoop);
        assertEquals(1, configurationResponse.getResponseCode());
        assertEquals("Success", configurationResponse.getResponseText());

        channel.shutdownNow();
    }

    @Test
    void failingTest() {
        ManagedChannel channel = ManagedChannelBuilder.forTarget("127.0.0.1:2000")
                .usePlaintext()
                .build();

        EventLoopServiceGrpc.EventLoopServiceBlockingStub eventLoopService = EventLoopServiceGrpc.newBlockingStub(channel);
        Configuration.EventLoop eventLoop = Configuration.EventLoop.newBuilder()
                .setProfileName("Meow")
                .setParentWorkers(0)
                .setChildWorkers(4)
                .build();

        Configuration.ConfigurationResponse configurationResponse = eventLoopService.eventLoop(eventLoop);
        assertEquals(-1, configurationResponse.getResponseCode());

        channel.shutdownNow();
    }
}
