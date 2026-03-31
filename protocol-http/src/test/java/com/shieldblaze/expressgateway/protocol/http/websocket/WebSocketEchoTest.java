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
package com.shieldblaze.expressgateway.protocol.http.websocket;

import com.shieldblaze.expressgateway.backend.NodeBuilder;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterBuilder;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.HTTPRoundRobin;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.sessionpersistence.NOOPSessionPersistence;
import com.shieldblaze.expressgateway.common.utils.AvailablePortUtil;
import com.shieldblaze.expressgateway.configuration.ConfigurationContext;
import com.shieldblaze.expressgateway.configuration.tls.TlsClientConfiguration;
import com.shieldblaze.expressgateway.configuration.tls.TlsServerConfiguration;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancer;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancerBuilder;
import okhttp3.OkHttpClient;
import okhttp3.Request;
import okhttp3.Response;
import okhttp3.WebSocket;
import okhttp3.WebSocketListener;
import org.jetbrains.annotations.NotNull;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.Assertions;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.Timeout;

import java.net.InetSocketAddress;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.TimeUnit;

@Timeout(value = 120, unit = TimeUnit.SECONDS)
public class WebSocketEchoTest extends WebSocketListener {

    private static final int MESSAGE_COUNT = 1_000;

    static WebSocketEchoServer webSocketEchoServer;
    static OkHttpClient client;
    static HTTPLoadBalancer httpLoadBalancer;
    static int lbPort;
    final CountDownLatch countDownLatchString = new CountDownLatch(MESSAGE_COUNT);

    @BeforeAll
    static void setup() throws Exception {
        webSocketEchoServer = new WebSocketEchoServer();
        webSocketEchoServer.startServer();

        client = new OkHttpClient();

        lbPort = AvailablePortUtil.getTcpPort();

        TlsClientConfiguration tlsClientConfiguration = TlsClientConfiguration.copyFrom(TlsClientConfiguration.DEFAULT);
        TlsServerConfiguration tlsServerConfiguration = TlsServerConfiguration.copyFrom(TlsServerConfiguration.DEFAULT);

        httpLoadBalancer = HTTPLoadBalancerBuilder.newBuilder()
                .withConfigurationContext(ConfigurationContext.create(tlsClientConfiguration, tlsServerConfiguration))
                .withBindAddress(new InetSocketAddress("127.0.0.1", lbPort))
                .build();

        httpLoadBalancer.start().future().get(30, TimeUnit.SECONDS);

        Cluster cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new HTTPRoundRobin(NOOPSessionPersistence.INSTANCE))
                .build();

        httpLoadBalancer.mappedCluster("localhost:" + lbPort, cluster);

        NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress("127.0.0.1", webSocketEchoServer.port()))
                .build();
    }

    @AfterAll
    static void shutdown() throws ExecutionException, InterruptedException, java.util.concurrent.TimeoutException {
        webSocketEchoServer.shutdown();
        client.dispatcher().executorService().shutdown();
        httpLoadBalancer.stop().future().get(30, TimeUnit.SECONDS);
    }

    @Test
    void testEcho() throws InterruptedException {
        Request request = new Request.Builder()
                .url("ws://localhost:" + lbPort)
                .build();

        client.newWebSocket(request, this);

        Assertions.assertTrue(countDownLatchString.await(90, TimeUnit.SECONDS), "WebSocket echo did not complete within 90 seconds");
    }

    @Override
    public void onOpen(@NotNull WebSocket webSocket, @NotNull Response response) {
        for (int i = 0; i < MESSAGE_COUNT; i++) {
            webSocket.send("Hello");
        }
    }

    @Override
    public void onMessage(@NotNull WebSocket webSocket, @NotNull String text) {
        Assertions.assertEquals("Hello", text);
        countDownLatchString.countDown();
    }
}
