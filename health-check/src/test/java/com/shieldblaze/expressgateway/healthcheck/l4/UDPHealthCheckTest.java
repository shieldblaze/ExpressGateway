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

import java.net.DatagramPacket;
import java.net.DatagramSocket;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.net.SocketException;
import java.util.Arrays;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;

final class UDPHealthCheckTest {

    static final byte[] PING = "PING".getBytes();
    static final byte[] PONG = "PONG".getBytes();

    @Test
    void check() throws InterruptedException {
        UDPServer udpServer = new UDPServer(true);
        udpServer.start();
        Thread.sleep(2500L); // Wait for UDP Server to Start

        UDPHealthCheck udpHealthCheck = new UDPHealthCheck( new InetSocketAddress("127.0.0.1", 9111), 5);
        udpHealthCheck.check();

        assertEquals(Health.GOOD, udpHealthCheck.health());
    }

    @Test
    void checkPong() throws InterruptedException {
        UDPServer udpServer = new UDPServer(false);
        udpServer.start();
        Thread.sleep(2500L); // Wait for UDP Server to Start

        UDPHealthCheck udpHealthCheck = new UDPHealthCheck( new InetSocketAddress("127.0.0.1", 9111), 5);
        udpHealthCheck.check();

        assertEquals(Health.GOOD, udpHealthCheck.health());
    }

    private static final class UDPServer extends Thread {

        private final boolean ping;

        private UDPServer(boolean ping) {
            this.ping = ping;
        }

        @Override
        public void run() {
            try (DatagramSocket datagramSocket = new DatagramSocket(9111, InetAddress.getByName("127.0.0.1"))) {
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
                ex.printStackTrace();
            }
        }
    }
}
