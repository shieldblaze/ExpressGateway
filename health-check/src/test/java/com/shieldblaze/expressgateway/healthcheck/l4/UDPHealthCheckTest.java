package com.shieldblaze.expressgateway.healthcheck.l4;

import org.junit.jupiter.api.Test;

import java.net.DatagramPacket;
import java.net.DatagramSocket;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.net.SocketException;
import java.util.Arrays;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;

class UDPHealthCheckTest {

    static final byte[] PING = "PING".getBytes();
    static final byte[] PONG = "PONG".getBytes();

    @Test
    void check() throws InterruptedException {
        UDPServer udpServer = new UDPServer(true);
        udpServer.start();
        Thread.sleep(2500L); // Wait for UDP Server to Start

        try (DatagramSocket datagramSocket = new DatagramSocket()) {
            datagramSocket.setSoTimeout(5000);
            DatagramPacket datagramPacket = new DatagramPacket(PING, 4, new InetSocketAddress("127.0.0.1", 9111));
            datagramSocket.send(datagramPacket);

            byte[] bytes = new byte[2048];
            datagramPacket = new DatagramPacket(bytes, bytes.length);
            datagramSocket.receive(datagramPacket);

            assertArrayEquals(PING, Arrays.copyOf(datagramPacket.getData(), datagramPacket.getLength()));
        } catch (Exception e) {
            e.printStackTrace();
        }
    }

    @Test
    void checkPong() throws InterruptedException {
        UDPServer udpServer = new UDPServer(false);
        udpServer.start();
        Thread.sleep(2500L); // Wait for UDP Server to Start

        try (DatagramSocket datagramSocket = new DatagramSocket()) {
            datagramSocket.setSoTimeout(5000);
            DatagramPacket datagramPacket = new DatagramPacket(PING, 4, new InetSocketAddress("127.0.0.1", 9111));
            datagramSocket.send(datagramPacket);

            byte[] bytes = new byte[2048];
            datagramPacket = new DatagramPacket(bytes, bytes.length);
            datagramSocket.receive(datagramPacket);

            assertArrayEquals(PONG, Arrays.copyOf(datagramPacket.getData(), datagramPacket.getLength()));
        } catch (Exception e) {
            e.printStackTrace();
        }
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
