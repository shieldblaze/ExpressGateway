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
import com.shieldblaze.expressgateway.core.cluster.CoreContext;
import com.shieldblaze.expressgateway.core.cluster.LoadBalancerContext;
import okhttp3.MediaType;
import okhttp3.Request;
import okhttp3.RequestBody;
import okhttp3.Response;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.MethodOrderer;
import org.junit.jupiter.api.Order;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.TestMethodOrder;
import reactor.netty.Connection;
import reactor.netty.DisposableServer;
import reactor.netty.tcp.TcpServer;
import reactor.netty.udp.UdpServer;

import java.io.File;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.net.Socket;
import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.util.Random;
import java.util.SplittableRandom;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;

import static com.shieldblaze.expressgateway.common.utils.SystemPropertyUtil.getPropertyOrEnv;
import static org.assertj.core.api.Assertions.assertThat;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

@TestMethodOrder(MethodOrderer.OrderAnnotation.class)
public class BasicTcpUdpServerTest {

    private static final Random RANDOM = new Random();

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
                .port(55555)
                .handle((nettyInbound, nettyOutbound) -> nettyOutbound.send(nettyInbound.receive().retain()))
                .bindNow();

        udpServer = UdpServer.create()
                .port(55555)
                .handle((udpInbound, udpOutbound) -> udpOutbound.send(udpInbound.receive().retain()))
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
        requestBody.addProperty("bindPort", 12345);
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
        requestBody.addProperty("bindPort", 12345);
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
        requestBody.addProperty("Hostname", "www.shieldblaze.com"); // It will default down to 'DEFAULT'.
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
        body.addProperty("port", 55555);

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
        body.addProperty("port", 55555);

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
    void sendTcpAndReceive() throws Exception {
        final CountDownLatch latch = new CountDownLatch(10);

        for (int threads = 0; threads < 10; threads++) {

            new Thread(() -> {
                try (Socket socket = new Socket("127.0.0.1", 12345)) {
                    InputStream inputStream = socket.getInputStream();
                    OutputStream outputStream = socket.getOutputStream();

                    final int dataSize = 128;

                    for (int i = 0; i < 10_000; i++) {
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

        latch.await(1, TimeUnit.MINUTES);
    }
}
