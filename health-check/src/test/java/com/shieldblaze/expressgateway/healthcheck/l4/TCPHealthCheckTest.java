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
import org.junit.jupiter.api.Test;

import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.net.ServerSocket;

import static org.junit.jupiter.api.Assertions.assertEquals;

final class TCPHealthCheckTest {

    @Test
    void check() throws InterruptedException {
        TCPServer tcpServer = new TCPServer();
        tcpServer.start();

        Thread.sleep(2500L); // Wait for TCP Server to Start

        TCPHealthCheck tcpHealthCheck = new TCPHealthCheck(new InetSocketAddress("127.0.0.1", 9111), 5);
        tcpHealthCheck.check();

        assertEquals(Health.GOOD, tcpHealthCheck.health());

        tcpServer.interrupt();
    }

    private static final class TCPServer extends Thread {

        @Override
        public void run() {
            try (ServerSocket serverSocket = new ServerSocket(9111, 1000, InetAddress.getByName("127.0.0.1"))) {
                serverSocket.accept();
            } catch (Exception ex) {
                ex.printStackTrace();
            }
        }
    }
}
