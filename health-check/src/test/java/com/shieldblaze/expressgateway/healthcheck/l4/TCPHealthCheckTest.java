package com.shieldblaze.expressgateway.healthcheck.l4;

import com.shieldblaze.expressgateway.healthcheck.Health;
import org.junit.jupiter.api.Test;

import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.net.ServerSocket;

import static org.junit.jupiter.api.Assertions.assertEquals;

class TCPHealthCheckTest {

    @Test
    void check() throws InterruptedException {
        TCPServer tcpServer = new TCPServer();
        tcpServer.start();

        Thread.sleep(2500L); // Wait for TCP Server to Start

        TCPHealthCheck tcpHealthCheck = new TCPHealthCheck(new InetSocketAddress("127.0.0.1", 9111), 5);
        tcpHealthCheck.check();

        assertEquals(Health.GOOD, tcpHealthCheck.getHealth());
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
