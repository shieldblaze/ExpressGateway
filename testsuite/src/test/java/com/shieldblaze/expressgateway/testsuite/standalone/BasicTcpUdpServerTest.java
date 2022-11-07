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

import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import com.shieldblaze.expressgateway.bootstrap.Bootstrap;
import com.shieldblaze.expressgateway.common.utils.AvailablePortUtil;
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
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.net.DatagramSocket;
import java.net.InetSocketAddress;
import java.net.Socket;
import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.util.Random;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;

import static com.shieldblaze.expressgateway.common.utils.SystemPropertyUtil.getPropertyOrEnv;
import static org.assertj.core.api.Assertions.assertThat;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

@TestMethodOrder(MethodOrderer.OrderAnnotation.class)
public class BasicTcpUdpServerTest {

    private static final Random RANDOM = new Random();

    private static int BackendTcpNodePort = AvailablePortUtil.getTcpPort();
    private static int BackendUdpNodePort = AvailablePortUtil.getUdpPort();

    private static int LoadBalancerTcpPort = AvailablePortUtil.getTcpPort();
    private static int LoadBalancerUdpPort = AvailablePortUtil.getUdpPort();

    private static String tcpId;
    private static String udpId;
    private static String tcpNodeId;
    private static String udpNodeId;

    private static DisposableServer tcpServer;
    private static Connection udpServer;

    @BeforeAll
    static void setup() throws Exception {
        assertNull(getPropertyOrEnv("CONFIGURATION_DIRECTORY"));

        ClassLoader classLoader = BasicTcpUdpServerTest.class.getClassLoader();
        File file = new File(classLoader.getResource("").getFile());
        String absolutePath = file.getAbsolutePath();

        System.setProperty("CONFIGURATION_FILE_NAME", "BasicTcpUdpServerTest.json");
        System.setProperty("CONFIGURATION_DIRECTORY", absolutePath);
        assertNotNull(getPropertyOrEnv("CONFIGURATION_DIRECTORY"));

        Bootstrap.main();

        tcpServer = TcpServer.create()
                .port(BackendTcpNodePort)
                .handle((nettyInbound, nettyOutbound) -> nettyOutbound.send(nettyInbound.receive().retain()))
                .bindNow();

        udpServer = UdpServer.create()
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
    static void shutdown() {
        Bootstrap.shutdown();

        if (tcpServer != null) {
            tcpServer.disposeNow();
        }

        if (udpServer != null) {
            udpServer.disposeNow();
        }
    }

    @Order(1)
    @Test
    public void startTcpLoadBalancer() throws IOException, InterruptedException {
        JsonObject requestBody = new JsonObject();
        requestBody.addProperty("name", "MeowBalancer");
        requestBody.addProperty("bindAddress", "127.0.0.1");
        requestBody.addProperty("bindPort", LoadBalancerTcpPort);
        requestBody.addProperty("protocol", "tcp");

        HttpRequest httpRequest = HttpRequest.newBuilder()
                .uri(URI.create("http://127.0.0.1:54321/v1/loadbalancer/l4/start"))
                .POST(HttpRequest.BodyPublishers.ofString(requestBody.toString()))
                .header("Content-Type", "application/json")
                .version(HttpClient.Version.HTTP_1_1)
                .build();

        HttpResponse<String> httpResponse = HttpClient.newHttpClient().send(httpRequest, HttpResponse.BodyHandlers.ofString());
        assertThat(httpResponse.statusCode()).isEqualTo(201);

        JsonObject responseJson = JsonParser.parseString(httpResponse.body()).getAsJsonObject();
        System.out.println(responseJson);
        assertTrue(responseJson.get("Success").getAsBoolean());

        tcpId = responseJson.get("Result").getAsJsonObject().get("LoadBalancerID").getAsString();
        System.err.println(tcpId);
    }

    @Order(2)
    @Test
    public void startUdpLoadBalancer() throws IOException, InterruptedException {
        JsonObject requestBody = new JsonObject();
        requestBody.addProperty("name", "MeowBalancer");
        requestBody.addProperty("bindAddress", "127.0.0.1");
        requestBody.addProperty("bindPort", LoadBalancerUdpPort);
        requestBody.addProperty("protocol", "udp");

        HttpRequest httpRequest = HttpRequest.newBuilder()
                .uri(URI.create("http://127.0.0.1:54321/v1/loadbalancer/l4/start"))
                .POST(HttpRequest.BodyPublishers.ofString(requestBody.toString()))
                .header("Content-Type", "application/json")
                .version(HttpClient.Version.HTTP_1_1)
                .build();

        HttpResponse<String> httpResponse = HttpClient.newHttpClient().send(httpRequest, HttpResponse.BodyHandlers.ofString());
        assertThat(httpResponse.statusCode()).isEqualTo(201);

        JsonObject responseJson = JsonParser.parseString(httpResponse.body()).getAsJsonObject();
        System.out.println(responseJson);
        assertTrue(responseJson.get("Success").getAsBoolean());

        udpId = responseJson.get("Result").getAsJsonObject().get("LoadBalancerID").getAsString();
    }

    @Order(3)
    @Test
    public void createTcpL4Cluster() throws Exception {
        JsonObject requestBody = new JsonObject();
        requestBody.addProperty("Hostname", "www.shieldblaze.com"); // It will default down to 'DEFAULT'.
        requestBody.addProperty("LoadBalance", "RoundRobin");
        requestBody.addProperty("SessionPersistence", "NOOP");

        HttpRequest httpRequest = HttpRequest.newBuilder()
                .uri(URI.create("http://127.0.0.1:54321/v1/cluster/create?id=" + tcpId))
                .POST(HttpRequest.BodyPublishers.ofString(requestBody.toString()))
                .header("Content-Type", "application/json")
                .version(HttpClient.Version.HTTP_1_1)
                .build();

        HttpResponse<String> httpResponse = HttpClient.newHttpClient().send(httpRequest, HttpResponse.BodyHandlers.ofString());
        assertThat(httpResponse.statusCode()).isEqualTo(201);

        JsonObject responseJson = JsonParser.parseString(httpResponse.body()).getAsJsonObject();
        System.out.println(responseJson);
        assertTrue(responseJson.get("Success").getAsBoolean());
    }

    @Order(4)
    @Test
    public void createUdpL4Cluster() throws Exception {
        JsonObject requestBody = new JsonObject();
        requestBody.addProperty("Hostname", "localhost"); // It will default down to 'DEFAULT'.
        requestBody.addProperty("LoadBalance", "RoundRobin");
        requestBody.addProperty("SessionPersistence", "NOOP");

        HttpRequest httpRequest = HttpRequest.newBuilder()
                .uri(URI.create("http://127.0.0.1:54321/v1/cluster/create?id=" + udpId))
                .POST(HttpRequest.BodyPublishers.ofString(requestBody.toString()))
                .header("Content-Type", "application/json")
                .version(HttpClient.Version.HTTP_1_1)
                .build();

        HttpResponse<String> httpResponse = HttpClient.newHttpClient().send(httpRequest, HttpResponse.BodyHandlers.ofString());
        assertThat(httpResponse.statusCode()).isEqualTo(201);

        JsonObject responseJson = JsonParser.parseString(httpResponse.body()).getAsJsonObject();
        System.out.println(responseJson);
        assertTrue(responseJson.get("Success").getAsBoolean());
    }

    @Order(5)
    @Test
    void createTcpBackendNode() throws Exception {
        JsonObject body = new JsonObject();
        body.addProperty("address", "127.0.0.1");
        body.addProperty("port", BackendTcpNodePort);

        HttpRequest httpRequest = HttpRequest.newBuilder()
                .uri(URI.create("http://127.0.0.1:54321/v1/node/create?id=" + tcpId + "&clusterHostname=default"))
                .POST(HttpRequest.BodyPublishers.ofString(body.toString()))
                .header("Content-Type", "application/json")
                .version(HttpClient.Version.HTTP_1_1)
                .build();

        HttpResponse<String> httpResponse = HttpClient.newHttpClient().send(httpRequest, HttpResponse.BodyHandlers.ofString());
        assertThat(httpResponse.statusCode()).isEqualTo(201);

        JsonObject responseJson = JsonParser.parseString(httpResponse.body()).getAsJsonObject();
        System.out.println(responseJson);

        tcpNodeId = responseJson.get("Result").getAsJsonObject().get("NodeID").getAsString();
    }

    @Order(6)
    @Test
    void createUdpBackendNode() throws Exception {
        JsonObject body = new JsonObject();
        body.addProperty("address", "127.0.0.1");
        body.addProperty("port", BackendUdpNodePort);

        HttpRequest httpRequest = HttpRequest.newBuilder()
                .uri(URI.create("http://127.0.0.1:54321/v1/node/create?id=" + udpId + "&clusterHostname=default"))
                .POST(HttpRequest.BodyPublishers.ofString(body.toString()))
                .header("Content-Type", "application/json")
                .version(HttpClient.Version.HTTP_1_1)
                .build();

        HttpResponse<String> httpResponse = HttpClient.newHttpClient().send(httpRequest, HttpResponse.BodyHandlers.ofString());
        assertThat(httpResponse.statusCode()).isEqualTo(201);

        JsonObject responseJson = JsonParser.parseString(httpResponse.body()).getAsJsonObject();
        System.out.println(responseJson);

        udpNodeId = responseJson.get("Result").getAsJsonObject().get("NodeID").getAsString();
    }

    @Order(7)
    @Test
    void sendTcpTrafficInMultiplexingWay() throws Exception {
        final int threads = 10;
        final int dataSize = 128;
        final CountDownLatch latch = new CountDownLatch(threads);

        for (int i = 0; i < threads; i++) {

            new Thread(() -> {
                try (Socket socket = new Socket("127.0.0.1", LoadBalancerTcpPort)) {
                    InputStream inputStream = socket.getInputStream();
                    OutputStream outputStream = socket.getOutputStream();

                    for (int messagesCount = 0; messagesCount < 10_000; messagesCount++) {
                        byte[] randomData = new byte[dataSize];
                        RANDOM.nextBytes(randomData);

                        outputStream.write(randomData);
                        outputStream.flush();

                        assertThat(inputStream.readNBytes(dataSize)).isEqualTo(randomData);
                    }
                } catch (Exception ex) {
                    throw new RuntimeException(ex);
                } finally {
                    latch.countDown();
                }
            }).start();
        }

        assertThat(latch.await(1, TimeUnit.MINUTES)).isTrue();
    }

    @Order(8)
    @Test
    void sendUdpTrafficInMultiplexingWay() throws Exception {
        final int threads = 10;
        final int dataSize = 128;
        final InetSocketAddress address = new InetSocketAddress("127.0.0.1", LoadBalancerUdpPort);
        final CountDownLatch latch = new CountDownLatch(threads);

        for (int i = 0; i < threads; i++) {

            new Thread(() -> {
                try (DatagramSocket socket = new DatagramSocket()) {

                    for (int messagesCount = 0; messagesCount < 10_000; messagesCount++) {
                        byte[] randomData = new byte[dataSize];
                        RANDOM.nextBytes(randomData);

                        java.net.DatagramPacket outboundPacket = new java.net.DatagramPacket(randomData, dataSize, address);
                        socket.send(outboundPacket);

                        byte[] buffer = new byte[dataSize];
                        java.net.DatagramPacket inboundPacket = new java.net.DatagramPacket(buffer, dataSize);
                        socket.receive(inboundPacket);

                        assertThat(buffer).isEqualTo(randomData);
                    }
                } catch (Exception ex) {
                    throw new RuntimeException(ex);
                } finally {
                    latch.countDown();
                }
            }).start();
        }

        assertThat(latch.await(1, TimeUnit.MINUTES)).isTrue();
    }
}
