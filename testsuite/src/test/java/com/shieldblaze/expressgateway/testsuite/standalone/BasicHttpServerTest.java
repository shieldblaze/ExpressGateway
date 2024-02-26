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

import com.shieldblaze.expressgateway.backend.NodeBuilder;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterBuilder;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.HTTPRoundRobin;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.sessionpersistence.NOOPSessionPersistence;
import com.shieldblaze.expressgateway.common.utils.AvailablePortUtil;
import com.shieldblaze.expressgateway.core.cluster.CoreContext;
import com.shieldblaze.expressgateway.core.cluster.LoadBalancerContext;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStartupEvent;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancer;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancerBuilder;
import com.shieldblaze.expressgateway.protocol.tcp.TCPListener;
import okhttp3.OkHttpClient;
import okhttp3.Request;
import okhttp3.Response;
import okhttp3.WebSocket;
import okhttp3.WebSocketListener;
import okio.ByteString;
import org.jetbrains.annotations.NotNull;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.MethodOrderer;
import org.junit.jupiter.api.Order;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.TestMethodOrder;
import reactor.core.publisher.Mono;
import reactor.netty.DisposableServer;
import reactor.netty.http.server.HttpServer;

import java.net.InetSocketAddress;
import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.time.Duration;
import java.util.Random;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.function.Supplier;

import static org.assertj.core.api.Assertions.assertThat;

@TestMethodOrder(MethodOrderer.OrderAnnotation.class)
public class BasicHttpServerTest {

    private static final Random RANDOM = new Random();
    private static final HttpClient HTTP_CLIENT = HttpClient.newHttpClient();
    private static final OkHttpClient OK_HTTP_CLIENT = new OkHttpClient();

    private static final int BackendTcpNodePort = AvailablePortUtil.getTcpPort();
    private static final int LoadBalancerTcpPort = AvailablePortUtil.getTcpPort();
    private static final AtomicInteger GET_REQUESTS = new AtomicInteger();
    private static final AtomicInteger POST_REQUESTS = new AtomicInteger();
    private static final AtomicInteger WEBSOCKET_FRAMES = new AtomicInteger();
    private static HTTPLoadBalancer httpLoadBalancer;
    private static DisposableServer server;

    @BeforeAll
    static void setup() {
        server = HttpServer.create()
                .bindAddress((Supplier<InetSocketAddress>) () -> new InetSocketAddress("127.0.0.1", BackendTcpNodePort))
                .route(routes -> routes
                        .get("/get", (request, response) -> response.sendString(Mono.just("Welcome to ShieldBlaze!")))
                        .post("/post", (request, response) -> response.send(request.receive().retain()))
                        .ws("/ws", (wsInbound, wsOutbound) -> wsOutbound.send(wsInbound.receive().retain())))
                .bindNow();
    }

    @AfterAll
    static void shutdown() {
        if (server != null) {
            server.disposeNow();
        }
    }

    @Order(1)
    @Test
    public void startL7LoadBalancer() throws Exception {
        httpLoadBalancer = HTTPLoadBalancerBuilder.newBuilder()
                .withBindAddress(new InetSocketAddress("127.0.0.1", LoadBalancerTcpPort))
                .withL4FrontListener(new TCPListener())
                .build();

        L4FrontListenerStartupEvent event = httpLoadBalancer.start();
        CoreContext.add("default-http", new LoadBalancerContext(httpLoadBalancer, event));
    }

    @Order(2)
    @Test
    public void createTcpL4Cluster() {
        Cluster tcpCluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new HTTPRoundRobin(NOOPSessionPersistence.INSTANCE))
                .build();

        CoreContext.get("default-http").l4LoadBalancer().defaultCluster(tcpCluster);
    }

    @Order(3)
    @Test
    public void createTcpBackendNode() throws Exception {
        NodeBuilder.newBuilder()
                .withCluster(httpLoadBalancer.defaultCluster())
                .withSocketAddress(new InetSocketAddress("127.0.0.1", BackendTcpNodePort))
                .build()
                .maxConnections(1_000_000);
    }

    @Order(4)
    @Test
    public void sendHttp11AndWebSocketsMultiplexingTraffic() throws Exception {
        assertThat(GET_REQUESTS.get()).isEqualTo(0);
        assertThat(POST_REQUESTS.get()).isEqualTo(0);
        assertThat(WEBSOCKET_FRAMES.get()).isEqualTo(0);

        final int frames = 10_000;
        final int threads = 10;
        final int dataSize = 128;
        final CountDownLatch latch = new CountDownLatch(threads * 3); // x3 because we run 3 test methods: GET, POST and WebSocket.

        for (int i = 0; i < threads; i++) {

            // Send GET requests
            new Thread(() -> {
                try {
                    HttpRequest httpRequest = HttpRequest.newBuilder()
                            .GET()
                            .uri(URI.create("http://127.0.0.1:" + LoadBalancerTcpPort + "/get"))
                            .version(HttpClient.Version.HTTP_1_1)
                            .timeout(Duration.ofSeconds(5))
                            .build();

                    for (int messagesCount = 0; messagesCount < frames; messagesCount++) {
                        HttpResponse<Void> httpResponse = HTTP_CLIENT.send(httpRequest, HttpResponse.BodyHandlers.discarding());
                        assertThat(httpResponse.statusCode()).isEqualTo(200);

                        GET_REQUESTS.incrementAndGet();
                    }
                } catch (Exception ex) {
                    throw new RuntimeException(ex);
                } finally {
                    latch.countDown();
                }
            }).start();

            // Send POST requests
            new Thread(() -> {
                try {
                    for (int messagesCount = 0; messagesCount < frames; messagesCount++) {
                        byte[] randomData = new byte[dataSize];
                        RANDOM.nextBytes(randomData);

                        HttpRequest httpRequest = HttpRequest.newBuilder()
                                .POST(HttpRequest.BodyPublishers.ofByteArray(randomData))
                                .uri(URI.create("http://127.0.0.1:" + LoadBalancerTcpPort + "/post"))
                                .version(HttpClient.Version.HTTP_1_1)
                                .timeout(Duration.ofSeconds(5))
                                .build();

                        HttpResponse<byte[]> httpResponse = HTTP_CLIENT.send(httpRequest, HttpResponse.BodyHandlers.ofByteArray());
                        assertThat(httpResponse.statusCode()).isEqualTo(200);
                        assertThat(httpResponse.body()).isEqualTo(randomData);

                        POST_REQUESTS.incrementAndGet();
                    }
                } catch (Exception ex) {
                    throw new RuntimeException(ex);
                } finally {
                    latch.countDown();
                }
            }).start();

            // Send WEBSOCKET frames
            new Thread(() -> {
                try {
                    Request request = new Request.Builder()
                            .url("ws://127.0.0.1:" + LoadBalancerTcpPort + "/ws")
                            .build();

                    OK_HTTP_CLIENT.newWebSocket(request, new WebSocketListener() {

                        private int framesSent;
                        private final byte[] randomData = new byte[dataSize];

                        @Override
                        public void onMessage(@NotNull WebSocket webSocket, @NotNull ByteString bytes) {

                            // If frameSent is not zero then we have previously sent a frame.
                            // We will verify that frame now.
                            if (framesSent != 0) {
                                assertThat(bytes.toByteArray()).isEqualTo(randomData);
                                WEBSOCKET_FRAMES.incrementAndGet();
                            }

                            if (framesSent == frames) {
                                webSocket.close(1000, "Close");
                                latch.countDown();
                                return;
                            }

                            RANDOM.nextBytes(randomData);
                            webSocket.send(ByteString.of(randomData));

                            framesSent++;
                        }

                        @Override
                        public void onOpen(@NotNull WebSocket webSocket, @NotNull Response response) {
                            // This byte message will trigger 'onMessage(ByteString)'.
                            webSocket.send(ByteString.of("Hello!".getBytes()));
                        }

                        @Override
                        public void onMessage(@NotNull WebSocket webSocket, @NotNull String text) {
                            latch.countDown();
                            super.onMessage(webSocket, text);
                        }
                    });
                } catch (Exception ex) {
                    throw new RuntimeException(ex);
                }
            }).start();
        }

        assertThat(latch.await(5, TimeUnit.MINUTES)).isTrue();
        assertThat(GET_REQUESTS.getAndSet(0)).isEqualTo(frames * threads);
        assertThat(POST_REQUESTS.getAndSet(0)).isEqualTo(frames * threads);
        assertThat(WEBSOCKET_FRAMES.getAndSet(0)).isEqualTo(frames * threads);
    }
}
