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
package com.shieldblaze.expressgateway.healthcheck.l4;

import com.shieldblaze.expressgateway.healthcheck.Health;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

import java.io.IOException;
import java.net.InetSocketAddress;
import java.nio.channels.AsynchronousServerSocketChannel;
import java.time.Duration;

import static org.junit.jupiter.api.Assertions.assertEquals;

final class TCPHealthCheckTest {

    static final TCPServer tcpServer = new TCPServer();

    @BeforeAll
    static void startTCPServer() throws IOException {
        tcpServer.start();
    }

    @AfterAll
    static void stopTCPServer() throws IOException {
        tcpServer.stop();
    }

    @Test
    void check() {
        TCPHealthCheck tcpHealthCheck = new TCPHealthCheck(new InetSocketAddress("127.0.0.1", 10000), Duration.ofSeconds(5));
        tcpHealthCheck.run();

        assertEquals(Health.GOOD, tcpHealthCheck.health());
    }

    private static final class TCPServer {

        private AsynchronousServerSocketChannel server;

        private void start() throws IOException {
            server = AsynchronousServerSocketChannel.open();
            server.bind(new InetSocketAddress("127.0.0.1", 10000));
            server.accept();
        }

        private void stop() throws IOException {
            server.close();
        }
    }
}
