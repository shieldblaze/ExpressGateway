package com.shieldblaze.expressgateway.core.server.udp;

import com.shieldblaze.expressgateway.core.L4LoadBalancer;
import com.shieldblaze.expressgateway.core.L4LoadBalancerBuilder;
import com.shieldblaze.expressgateway.core.configuration.Configuration;
import com.shieldblaze.expressgateway.core.configuration.ConfigurationBuilder;
import com.shieldblaze.expressgateway.core.configuration.buffer.PooledByteBufAllocatorConfiguration;
import com.shieldblaze.expressgateway.core.configuration.eventloop.EventLoopConfiguration;
import com.shieldblaze.expressgateway.core.configuration.eventloop.EventLoopConfigurationBuilder;
import com.shieldblaze.expressgateway.core.configuration.transport.ReceiveBufferAllocationType;
import com.shieldblaze.expressgateway.core.configuration.transport.TransportConfiguration;
import com.shieldblaze.expressgateway.core.configuration.transport.TransportConfigurationBuilder;
import com.shieldblaze.expressgateway.core.configuration.transport.TransportType;
import com.shieldblaze.expressgateway.core.loadbalance.backend.Backend;
import com.shieldblaze.expressgateway.core.loadbalance.backend.Cluster;
import com.shieldblaze.expressgateway.core.loadbalance.l4.RoundRobin;
import com.shieldblaze.expressgateway.core.netty.EventLoopFactory;
import io.netty.channel.epoll.Epoll;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

import java.io.DataInputStream;
import java.io.DataOutputStream;
import java.net.DatagramPacket;
import java.net.DatagramSocket;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.net.Socket;
import java.util.Arrays;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

class UpstreamHandlerTest {

    static L4LoadBalancer l4LoadBalancer;
    static EventLoopFactory eventLoopFactory;

    @BeforeAll
    static void setup() {

        TransportConfiguration transportConfiguration = TransportConfigurationBuilder.newBuilder()
                .withTransportType(Epoll.isAvailable() ? TransportType.EPOLL : TransportType.NIO)
                .withTCPFastOpenMaximumPendingRequests(2147483647)
                .withBackendConnectTimeout(1000 * 5)
                .withBackendSocketTimeout(1000 * 5)
                .withReceiveBufferAllocationType(ReceiveBufferAllocationType.FIXED)
                .withReceiveBufferSizes(new int[]{100})
                .withSocketReceiveBufferSize(2147483647)
                .withSocketSendBufferSize(2147483647)
                .withTCPConnectionBacklog(2147483647)
                .withDataBacklog(2147483647)
                .withConnectionIdleTimeout(180)
                .build();

        EventLoopConfiguration eventLoopConfiguration = EventLoopConfigurationBuilder.newBuilder()
                .withParentWorkers(Runtime.getRuntime().availableProcessors())
                .withChildWorkers(Runtime.getRuntime().availableProcessors() * 2)
                .build();

        Configuration configuration = ConfigurationBuilder.newBuilder()
                .withTransportConfiguration(transportConfiguration)
                .withEventLoopConfiguration(eventLoopConfiguration)
                .withPooledByteBufAllocatorConfiguration(PooledByteBufAllocatorConfiguration.DEFAULT)
                .build();

        eventLoopFactory = new EventLoopFactory(configuration);

        Cluster cluster = new Cluster();
        cluster.setClusterName("MyCluster");
        cluster.addBackend(new Backend(new InetSocketAddress("127.0.0.1", 9111)));

        l4LoadBalancer = L4LoadBalancerBuilder.newBuilder()
                .withConfiguration(configuration)
                .withL4Balance(new RoundRobin())
                .withCluster(cluster)
                .withFrontListener(new UDPListener(new InetSocketAddress("127.0.0.1", 9110)))
                .build();

        assertTrue(l4LoadBalancer.start());
        new UDPServer().start();
    }

    @AfterAll
    static void stop() {
        l4LoadBalancer.stop();
        assertFalse(l4LoadBalancer.hasStarted());
    }

    @Test
    void udpClient() throws Exception {
        try (DatagramSocket datagramSocket = new DatagramSocket()) {
            DatagramPacket datagramPacket = new DatagramPacket("HELLO_FROM_CLIENT".getBytes(), "HELLO_FROM_CLIENT".length(),
                    InetAddress.getByName("127.0.0.1"), 9110);

            datagramSocket.send(datagramPacket);
            byte[] bytes = new byte[2048];
            datagramPacket = new DatagramPacket(bytes, bytes.length);
            datagramSocket.receive(datagramPacket);

            assertEquals("HELLO_FROM_SERVER", new String(Arrays.copyOf(datagramPacket.getData(), datagramPacket.getLength())));
        }
    }

    private static final class UDPServer extends Thread {

        @Override
        public void run() {
            try (DatagramSocket datagramSocket = new DatagramSocket(9111, InetAddress.getByName("127.0.0.1"))) {
                byte[] bytes = new byte[2048];
                DatagramPacket datagramPacket = new DatagramPacket(bytes, bytes.length);
                datagramSocket.receive(datagramPacket);

                InetAddress inetAddress = datagramPacket.getAddress();
                int port = datagramPacket.getPort();

                assertEquals("HELLO_FROM_CLIENT", new String(Arrays.copyOf(datagramPacket.getData(), datagramPacket.getLength())));

                datagramPacket = new DatagramPacket("HELLO_FROM_SERVER".getBytes(), "HELLO_FROM_SERVER".length(), inetAddress, port);
                datagramSocket.send(datagramPacket);
            } catch (Exception ex) {
                ex.printStackTrace();
            }
        }
    }
}
