package com.shieldblaze.expressgateway.server;

import com.shieldblaze.expressgateway.netty.EventLoopUtils;
import com.shieldblaze.expressgateway.server.tcp.Server;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

import java.io.DataInputStream;
import java.io.DataOutputStream;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.net.ServerSocket;
import java.net.Socket;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

class UpstreamHandlerTest {

    static Server server;

    @BeforeAll
    static void setup() throws InterruptedException {
        server = new Server(new InetSocketAddress("127.0.0.1", 9110));
        server.start();
        assertTrue(server.channelFuture.sync().isSuccess());
        new TCPServer().start();
    }

    @AfterAll
    static void stop() throws InterruptedException {
        assertTrue(EventLoopUtils.PARENT.shutdownGracefully().sync().isSuccess());
        assertTrue(EventLoopUtils.CHILD.shutdownGracefully().sync().isSuccess());
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
