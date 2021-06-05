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
package com.shieldblaze.expressgateway.healthcheck.l4;

import com.shieldblaze.expressgateway.healthcheck.Health;
import org.junit.jupiter.api.Test;

import java.net.DatagramPacket;
import java.net.DatagramSocket;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.time.Duration;
import java.util.Arrays;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;

final class UDPHealthCheckTest {

    static final byte[] PING = "PING".getBytes();
    static final byte[] PONG = "PONG".getBytes();

    @Test
    void checkPing() throws InterruptedException {
        UDPServer udpServer = new UDPServer(true, 12345);
        udpServer.start();
        Thread.sleep(2500L); // Wait for UDP Server to Start

        UDPHealthCheck udpHealthCheck = new UDPHealthCheck(new InetSocketAddress("127.0.0.1", 12345), Duration.ofSeconds(5));
        udpHealthCheck.run();

        assertEquals(Health.GOOD, udpHealthCheck.health());
    }

    @Test
    void checkPong() throws InterruptedException {
        UDPServer udpServer = new UDPServer(false, 12346);
        udpServer.start();
        Thread.sleep(2500L); // Wait for UDP Server to Start

        UDPHealthCheck udpHealthCheck = new UDPHealthCheck(new InetSocketAddress("127.0.0.1", 12346), Duration.ofSeconds(5));
        udpHealthCheck.run();

        assertEquals(Health.GOOD, udpHealthCheck.health());
    }

    private static final class UDPServer extends Thread {

        private final boolean ping;
        private final int port;

        private UDPServer(boolean ping, int port) {
            this.ping = ping;
            this.port = port;
        }

        @Override
        public void run() {
            try (DatagramSocket datagramSocket = new DatagramSocket(port, InetAddress.getByName("127.0.0.1"))) {
                byte[] bytes = new byte[2048];
                DatagramPacket datagramPacket = new DatagramPacket(bytes, bytes.length);
                datagramSocket.receive(datagramPacket);

                InetAddress inetAddress = datagramPacket.getAddress();
                int port = datagramPacket.getPort();

                assertArrayEquals(PING, Arrays.copyOf(datagramPacket.getData(), datagramPacket.getLength()));

                if (ping) {
                    datagramPacket = new DatagramPacket(PING, 4, inetAddress, port);
                } else {
                    datagramPacket = new DatagramPacket(PONG, 4, inetAddress, port);
                }

                datagramSocket.send(datagramPacket);
            } catch (Exception ex) {
                // Ignore
            }
        }
    }
}
