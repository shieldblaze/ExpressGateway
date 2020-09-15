package com.shieldblaze.expressgateway.core.server;

import com.shieldblaze.expressgateway.core.loadbalance.backend.Backend;
import com.shieldblaze.expressgateway.core.loadbalance.l4.RoundRobin;
import com.shieldblaze.expressgateway.core.server.tcp.TCPListener;
import com.shieldblaze.expressgateway.core.netty.EventLoopFactory;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

import java.io.DataInputStream;
import java.io.DataOutputStream;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.net.ServerSocket;
import java.net.Socket;
import java.util.Collections;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

class UpstreamHandlerTest {

    static FrontListener listener;

    @BeforeAll
    static void setup() {
        listener = new TCPListener(new InetSocketAddress("127.0.0.1", 9110));
        listener.start(new RoundRobin(Collections.singletonList(new Backend(new InetSocketAddress("127.0.0.1", 9111)))));
        assertTrue(listener.waitForStart());
        new TCPServer().start();
    }

    @AfterAll
    static void stop() throws InterruptedException {
        listener.stop();
        assertTrue(EventLoopFactory.PARENT.shutdownGracefully().sync().isSuccess());
        assertTrue(EventLoopFactory.CHILD.shutdownGracefully().sync().isSuccess());
    }

    @Test
    void tcpClient() throws Exception {
        try (Socket client = new Socket("127.0.0.1", 9110)) {
            DataInputStream in = new DataInputStream(client.getInputStream());
            DataOutputStream out = new DataOutputStream(client.getOutputStream());

            out.writeUTF("HELLO_FROM_CLIENT");
            out.flush();

            assertEquals("HELLO_FROM_SERVER", in.readUTF());
        }
    }

    private static final class TCPServer extends Thread {

        @Override
        public void run() {
            try (ServerSocket serverSocket = new ServerSocket(9111, 1000, InetAddress.getByName("127.0.0.1"))) {
                Socket clientSocket = serverSocket.accept();
                DataInputStream input = new DataInputStream(clientSocket.getInputStream());
                DataOutputStream out = new DataOutputStream(clientSocket.getOutputStream());

                assertEquals("HELLO_FROM_CLIENT", input.readUTF());

                out.writeUTF("HELLO_FROM_SERVER");
                out.flush();
            } catch (Exception ex) {
                ex.printStackTrace();
            }
        }
    }
}
