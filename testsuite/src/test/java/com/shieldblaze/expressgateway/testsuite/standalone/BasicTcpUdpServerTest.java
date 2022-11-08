/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2022 ShieldBlaze
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
package com.shieldblaze.expressgateway.testsuite.standalone;

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.NodeBuilder;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterBuilder;
import com.shieldblaze.expressgateway.backend.strategy.l4.RoundRobin;
import com.shieldblaze.expressgateway.backend.strategy.l4.sessionpersistence.NOOPSessionPersistence;
import com.shieldblaze.expressgateway.common.utils.AvailablePortUtil;
import com.shieldblaze.expressgateway.configuration.ConfigurationContext;
import com.shieldblaze.expressgateway.core.cluster.CoreContext;
import com.shieldblaze.expressgateway.core.cluster.LoadBalancerContext;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStartupEvent;
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancer;
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancerBuilder;
import com.shieldblaze.expressgateway.protocol.tcp.TCPListener;
import com.shieldblaze.expressgateway.protocol.udp.UDPListener;
import io.netty.channel.socket.DatagramPacket;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.MethodOrderer;
import org.junit.jupiter.api.Order;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.TestMethodOrder;
import reactor.core.publisher.Mono;
import reactor.netty.Connection;
import reactor.netty.DisposableServer;
import reactor.netty.tcp.TcpServer;
import reactor.netty.udp.UdpServer;

import java.io.File;
import java.io.InputStream;
import java.io.OutputStream;
import java.net.DatagramSocket;
import java.net.InetSocketAddress;
import java.net.Socket;
import java.net.SocketTimeoutException;
import java.util.Random;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;

import static com.shieldblaze.expressgateway.common.utils.SystemPropertyUtil.getPropertyOrEnv;
import static org.assertj.core.api.Assertions.assertThat;
import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;

@TestMethodOrder(MethodOrderer.OrderAnnotation.class)
public class BasicTcpUdpServerTest {

    private static final Random RANDOM = new Random();

    private static final int BackendTcpNodePort = AvailablePortUtil.getTcpPort();
    private static final int BackendUdpNodePort = AvailablePortUtil.getUdpPort();

    private static final int LoadBalancerTcpPort = AvailablePortUtil.getTcpPort();
    private static final int LoadBalancerUdpPort = AvailablePortUtil.getUdpPort();

    private static L4LoadBalancer tcpLoadBalancer;
    private static L4LoadBalancer udpLoadBalancer;

    private static DisposableServer tcpServer;
    private static Connection udpServer;

    private static final AtomicInteger TCP_FRAMES = new AtomicInteger();
    private static final AtomicInteger UDP_FRAMES = new AtomicInteger();

    @BeforeAll
    static void setup() throws Exception {
        assertNull(getPropertyOrEnv("CONFIGURATION_DIRECTORY"));

        ClassLoader classLoader = BasicTcpUdpServerTest.class.getClassLoader();
        File file = new File(classLoader.getResource("").getFile());
        String absolutePath = file.getAbsolutePath();

        System.setProperty("CONFIGURATION_FILE_NAME", "BasicTcpUdpServerTest.json");
        System.setProperty("CONFIGURATION_DIRECTORY", absolutePath);
        assertNotNull(getPropertyOrEnv("CONFIGURATION_DIRECTORY"));

        tcpServer = TcpServer.create()
                .host("127.0.0.1")
                .port(BackendTcpNodePort)
                .handle((nettyInbound, nettyOutbound) -> nettyOutbound.send(nettyInbound.receive().retain()))
                .bindNow();

        udpServer = UdpServer.create()
                .host("127.0.0.1")
                .port(BackendUdpNodePort)
                .handle((in, out) -> out.sendObject(in.receiveObject()
                        .map(o -> {
                            if (o instanceof DatagramPacket packet) {
                                return new DatagramPacket(packet.content().retain(), packet.sender());
                            } else {
                                //noinspection ReactiveStreamsUnusedPublisher
                                return Mono.error(new Exception("Unexpected type of the message: " + o));
                            }
                        })))
                .bindNow();
    }

    @AfterAll
    static void shutdown() throws Exception {
        if (tcpLoadBalancer != null) {
            tcpLoadBalancer.shutdown().future().get();
        }

        if (udpLoadBalancer != null) {
            udpLoadBalancer.shutdown().future().get();
        }

        if (tcpServer != null) {
            tcpServer.disposeNow();
        }

        if (udpServer != null) {
            udpServer.disposeNow();
        }
    }

    @Order(1)
    @Test
    public void startTcpLoadBalancer() throws Exception {
        L4LoadBalancer l4LoadBalancer = L4LoadBalancerBuilder.newBuilder()
                .withL4FrontListener(new TCPListener())
                .withBindAddress(new InetSocketAddress("127.0.0.1", LoadBalancerTcpPort))
                .withCoreConfiguration(ConfigurationContext.DEFAULT)
                .build();

        tcpLoadBalancer = l4LoadBalancer;

        L4FrontListenerStartupEvent event = l4LoadBalancer.start();
        CoreContext.add("default-tcp", new LoadBalancerContext(l4LoadBalancer, event));
    }

    @Order(2)
    @Test
    public void startUdpLoadBalancer() throws Exception {
        L4LoadBalancer l4LoadBalancer = L4LoadBalancerBuilder.newBuilder()
                .withL4FrontListener(new UDPListener())
                .withBindAddress(new InetSocketAddress("127.0.0.1", LoadBalancerUdpPort))
                .withCoreConfiguration(ConfigurationContext.DEFAULT)
                .build();

        udpLoadBalancer = l4LoadBalancer;

        L4FrontListenerStartupEvent event = l4LoadBalancer.start();
        CoreContext.add("default-udp", new LoadBalancerContext(l4LoadBalancer, event));
    }

    @Order(3)
    @Test
    public void createTcpL4Cluster() throws Exception {
        Cluster tcpCluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new RoundRobin(NOOPSessionPersistence.INSTANCE))
                .build();

        CoreContext.get("default-tcp").l4LoadBalancer().defaultCluster(tcpCluster);
    }

    @Order(4)
    @Test
    public void createUdpL4Cluster() {
        Cluster udpCluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new RoundRobin(NOOPSessionPersistence.INSTANCE))
                .build();

        CoreContext.get("default-udp").l4LoadBalancer().defaultCluster(udpCluster);
    }

    @Order(5)
    @Test
    void createTcpBackendNode() throws Exception {
        NodeBuilder.newBuilder()
                .withCluster(tcpLoadBalancer.defaultCluster())
                .withSocketAddress(new InetSocketAddress("127.0.0.1", BackendTcpNodePort))
                .build();
    }

    @Order(6)
    @Test
    void createUdpBackendNode() throws Exception {
        NodeBuilder.newBuilder()
                .withCluster(udpLoadBalancer.defaultCluster())
                .withSocketAddress(new InetSocketAddress("127.0.0.1", BackendUdpNodePort))
                .build();
    }

    @Order(7)
    @Test
    void sendTcpTrafficInMultiplexingWay() throws Exception {
        assertThat(TCP_FRAMES.get()).isEqualTo(0);

        final int frames = 10_000;
        final int threads = 10;
        final int dataSize = 128;
        final CountDownLatch latch = new CountDownLatch(threads);

        for (int i = 0; i < threads; i++) {

            new Thread(() -> {
                try (Socket socket = new Socket("127.0.0.1", LoadBalancerTcpPort)) {
                    InputStream inputStream = socket.getInputStream();
                    OutputStream outputStream = socket.getOutputStream();

                    for (int messagesCount = 0; messagesCount < frames; messagesCount++) {
                        byte[] randomData = new byte[dataSize];
                        RANDOM.nextBytes(randomData);

                        outputStream.write(randomData);
                        outputStream.flush();

                        assertThat(inputStream.readNBytes(dataSize)).isEqualTo(randomData);
                        TCP_FRAMES.incrementAndGet();
                    }
                } catch (Exception ex) {
                    throw new RuntimeException(ex);
                } finally {
                    latch.countDown();
                }
            }).start();
        }

        assertThat(latch.await(1, TimeUnit.MINUTES)).isTrue();
        assertThat(TCP_FRAMES.getAndSet(0)).isEqualTo(frames * threads);
    }

    @Order(8)
    @Test
    void sendUdpTrafficInMultiplexingWay() throws Exception {
        assertThat(UDP_FRAMES.get()).isEqualTo(0);

        final int frames = 10_000;
        final int threads = 10;
        final int dataSize = 128;
        final InetSocketAddress address = new InetSocketAddress("127.0.0.1", LoadBalancerUdpPort);
        final CountDownLatch latch = new CountDownLatch(threads);

        for (int i = 0; i < threads; i++) {

            new Thread(() -> {
                try (DatagramSocket socket = new DatagramSocket()) {

                    for (int messagesCount = 0; messagesCount < frames; messagesCount++) {
                        byte[] randomData = new byte[dataSize];
                        RANDOM.nextBytes(randomData);

                        java.net.DatagramPacket outboundPacket = new java.net.DatagramPacket(randomData, dataSize, address);
                        socket.send(outboundPacket);

                        byte[] buffer = new byte[dataSize];
                        java.net.DatagramPacket inboundPacket = new java.net.DatagramPacket(buffer, dataSize);
                        socket.receive(inboundPacket);

                        assertThat(buffer).isEqualTo(randomData);
                        UDP_FRAMES.incrementAndGet();
                    }
                } catch (Exception ex) {
                    throw new RuntimeException(ex);
                } finally {
                    latch.countDown();
                }
            }).start();
        }

        assertThat(latch.await(1, TimeUnit.MINUTES)).isTrue();
        assertThat(UDP_FRAMES.getAndSet(0)).isEqualTo(frames * threads);
    }

    @Order(9)
    @Test
    void markTcpBackendOffline() {
        CoreContext.get("default-tcp").l4LoadBalancer()
                .defaultCluster()
                .onlineNodes()
                .get(0)
                .markOffline();
    }

    @Order(10)
    @Test
    void sendTcpTrafficOnOfflineBackend() throws Exception {
        try (Socket socket = new Socket()) {
            assertDoesNotThrow(() -> socket.connect(new InetSocketAddress("127.0.0.1", LoadBalancerTcpPort), 1000 * 10));
            assertThat(socket.isConnected()).isTrue();

            // Wait for 1 second for connection to be closed
            Thread.sleep(1000);

            assertThat(socket.getInputStream().read()).isEqualTo(-1);
        }
    }

    @Order(11)
    @Test
    void markUdpBackendOffline() throws Exception {
        CoreContext.get("default-udp").l4LoadBalancer()
                .defaultCluster()
                .onlineNodes()
                .get(0)
                .markOffline();
    }

    @Order(12)
    @Test
    void sendUdpTrafficOnOfflineBackend() throws Exception {
        try (DatagramSocket socket = new DatagramSocket()) {
            socket.setSoTimeout(1000 * 5);

            java.net.DatagramPacket packet = new java.net.DatagramPacket("Meow".getBytes(), 4, new InetSocketAddress("127.0.0.1", LoadBalancerUdpPort));
            assertDoesNotThrow(() -> socket.send(packet));

            byte[] data = new byte[4];
            java.net.DatagramPacket receivingPacket = new java.net.DatagramPacket(data, data.length);
            assertThrows(SocketTimeoutException.class, () -> socket.receive(receivingPacket));
        }
    }

    @Order(13)
    @Test
    void markTcpBackendOnline() {
        CoreContext.get("default-tcp").l4LoadBalancer()
                .defaultCluster()
                .allNodes()
                .get(0)
                .markOnline();
    }

    @Order(14)
    @Test
    void sendTcpTrafficInMultiplexingWayAfterMarkingOnline() throws Exception {
        sendTcpTrafficInMultiplexingWay();
    }

    @Order(15)
    @Test
    void markUdpBackendOnline() {
        CoreContext.get("default-udp").l4LoadBalancer()
                .defaultCluster()
                .allNodes()
                .get(0)
                .markOnline();
    }

    @Order(16)
    @Test
    void sendUdpTrafficInMultiplexingWayAfterMarkingOnline() throws Exception {
        sendUdpTrafficInMultiplexingWay();
    }

    @Order(17)
    @Test
    void sendTcpTrafficInMultiplexingWayAndMarkBackendOfflineWithoutDrainingConnection() throws Exception {
        CompletableFuture<Boolean> future = new CompletableFuture<>();

        new Thread(() -> {
            try {
                sendTcpTrafficInMultiplexingWay();
                future.complete(true);
            } catch (Exception e) {
                future.completeExceptionally(e);
                throw new RuntimeException(e);
            }
        }).start();

        Thread.sleep(1000);

        // Now mark
        CoreContext.get("default-tcp").l4LoadBalancer()
                .defaultCluster()
                .onlineNodes()
                .get(0)
                .markOffline();

        future.get(3, TimeUnit.MINUTES);
    }

    @Order(18)
    @Test
    void markTcpBackendOnlineAgain() {
        markTcpBackendOnline();
    }

    @Order(19)
    @Test
    void sendTcpTrafficInMultiplexingWayAndMarkBackendOfflineWithDrainingConnection() throws Exception {
        assertThat(TCP_FRAMES.get()).isEqualTo(0);

        final int frames = 10_000;
        final int threads = 10;
        final int dataSize = 128;
        final CountDownLatch latch = new CountDownLatch(threads);

        for (int i = 0; i < threads; i++) {

            new Thread(() -> {
                try (Socket socket = new Socket("127.0.0.1", LoadBalancerTcpPort)) {
                    InputStream inputStream = socket.getInputStream();
                    OutputStream outputStream = socket.getOutputStream();

                    for (int messagesCount = 0; messagesCount < frames; messagesCount++) {
                        byte[] randomData = new byte[dataSize];
                        RANDOM.nextBytes(randomData);

                        outputStream.write(randomData);
                        outputStream.flush();

                        assertThat(inputStream.readNBytes(dataSize)).isEqualTo(randomData);
                        TCP_FRAMES.incrementAndGet();
                    }
                } catch (Exception ex) {
                    throw new RuntimeException(ex);
                } finally {
                    latch.countDown();
                }
            }).start();
        }

        Thread.sleep(1000);

        // Mark the Backend offline and drain connections
        Node node = CoreContext.get("default-udp").l4LoadBalancer()
                .defaultCluster()
                .onlineNodes()
                .get(0);

        node.markOffline();
        node.drainConnections();

        assertThat(latch.await(1, TimeUnit.MINUTES)).isTrue();
        assertThat(TCP_FRAMES.getAndSet(0)).isBetween(1, (frames * threads));
    }
}
