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

class HTTPServiceTest {

    static Server server;

    @BeforeAll
    static void setup() throws IOException {
        System.setProperty("EGWConfDir", System.getProperty("java.io.tmpdir"));

        server = NettyServerBuilder.forAddress(new InetSocketAddress("127.0.0.1", 9110))
                .addService(new HTTPService())
                .build()
                .start();
    }

    @AfterAll
    static void shutdown() throws InterruptedException {
        server.shutdownNow().awaitTermination();
    }

    @Test
    void simpleTest() {
        ManagedChannel channel = ManagedChannelBuilder.forTarget("127.0.0.1:9110")
                .usePlaintext()
                .build();

        HTTPServiceGrpc.HTTPServiceBlockingStub httpService = HTTPServiceGrpc.newBlockingStub(channel);
        Configuration.HTTP http = Configuration.HTTP.newBuilder()
                .setProfileName("Meow")
                .setBrotliCompressionLevel(4)
                .setDeflateCompressionLevel(6)
                .setCompressionThreshold(91100)
                .setMaxChunkSize(10000)
                .setMaxHeaderSize(1000)
                .setMaxInitialLineLength(10000)
                .setH2MaxFrameSize(50000)
                .setH2MaxHeaderTableSize(1024)
                .setH2MaxHeaderListSize(256)
                .setH2MaxConcurrentStreams(100000)
                .setH2InitialWindowSize(65000)
                .setMaxContentLength(1000000000)
                .build();

        Configuration.ConfigurationResponse configurationResponse = httpService.http(http);
        assertEquals(1, configurationResponse.getResponseCode());
        assertEquals("Success", configurationResponse.getResponseText());

        channel.shutdownNow();
    }

    @Test
    void failingTest() {
        ManagedChannel channel = ManagedChannelBuilder.forTarget("127.0.0.1:9110")
                .usePlaintext()
                .build();

        HTTPServiceGrpc.HTTPServiceBlockingStub httpService = HTTPServiceGrpc.newBlockingStub(channel);
        Configuration.HTTP http = Configuration.HTTP.newBuilder()
                .setProfileName("Meow")
                .setBrotliCompressionLevel(1000) // 1-11 valid
                .setDeflateCompressionLevel(6)
                .setCompressionThreshold(91100)
                .setMaxChunkSize(10000)
                .setMaxHeaderSize(1000)
                .setMaxInitialLineLength(10000)
                .setH2MaxFrameSize(50000)
                .setH2MaxHeaderTableSize(1024)
                .setH2MaxHeaderListSize(256)
                .setH2MaxConcurrentStreams(100000)
                .setH2InitialWindowSize(65000)
                .setMaxContentLength(1000000000)
                .build();

        Configuration.ConfigurationResponse configurationResponse = httpService.http(http);
        assertEquals(-1, configurationResponse.getResponseCode());

        channel.shutdownNow();
    }
}
