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
package com.shieldblaze.expressgateway.protocol.http;

import com.shieldblaze.expressgateway.backend.NodeBuilder;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterBuilder;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.HTTPRoundRobin;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.sessionpersistence.NOOPSessionPersistence;
import com.shieldblaze.expressgateway.common.utils.AvailablePortUtil;
import com.shieldblaze.expressgateway.configuration.ConfigurationContext;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStartupTask;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancer;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancerBuilder;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

import java.io.BufferedReader;
import java.io.InputStreamReader;
import java.io.OutputStream;
import java.net.InetSocketAddress;
import java.net.Socket;
import java.nio.charset.StandardCharsets;

import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * P1 test: Verify that a request with headers exceeding {@code maxHeaderSize}
 * is rejected with a 431 Request Header Fields Too Large response (or 400 Bad Request).
 *
 * <p>Netty's {@link io.netty.handler.codec.http.HttpServerCodec} enforces header
 * size limits at the codec level. When headers exceed the configured max, the
 * codec produces a {@code DecoderResult.failure()} on the decoded request, which
 * the {@link Http11ServerInboundHandler} detects and rejects with 400 Bad Request.</p>
 *
 * <p>Per RFC 9110 Section 15.5.32, a server SHOULD respond with 431 when request
 * header fields are too large. Netty uses a TooLongHttpHeaderException which results
 * in the Http11ServerInboundHandler responding with 400 (the DecoderResult path).
 * Both 400 and 431 are acceptable responses for oversized headers.</p>
 */
class OversizedHeaderTest {

    private static int loadBalancerPort;
    private static HTTPLoadBalancer httpLoadBalancer;
    private static HttpServer httpServer;

    @BeforeAll
    static void setup() throws Exception {
        httpServer = new HttpServer(false);
        httpServer.start();
        httpServer.START_FUTURE.get();

        loadBalancerPort = AvailablePortUtil.getTcpPort();

        Cluster cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new HTTPRoundRobin(NOOPSessionPersistence.INSTANCE))
                .build();

        httpLoadBalancer = HTTPLoadBalancerBuilder.newBuilder()
                .withConfigurationContext(ConfigurationContext.DEFAULT)
                .withBindAddress(new InetSocketAddress("127.0.0.1", loadBalancerPort))
                .build();

        httpLoadBalancer.mappedCluster("localhost:" + loadBalancerPort, cluster);

        NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress("127.0.0.1", httpServer.port()))
                .build();

        L4FrontListenerStartupTask startupTask = httpLoadBalancer.start();
        startupTask.future().join();
        assertTrue(startupTask.isSuccess());
    }

    @AfterAll
    static void shutdown() throws Exception {
        httpLoadBalancer.shutdown().future().get();
        httpServer.shutdown();
        httpServer.SHUTDOWN_FUTURE.get();
    }

    /**
     * Send a request with a header value exceeding the default maxHeaderSize (8192 bytes).
     * The proxy should reject it with a 400 or 431 response.
     */
    @Test
    void oversizedHeaderReturns400Or431() throws Exception {
        // Default maxHeaderSize is 8192 bytes. Generate a header value larger than that.
        String largeValue = "X".repeat(16384); // 16 KB -- well over the 8 KB default

        String rawRequest = "GET / HTTP/1.1\r\n" +
                "Host: localhost:" + loadBalancerPort + "\r\n" +
                "X-Large-Header: " + largeValue + "\r\n" +
                "\r\n";

        String response = sendRawRequest(rawRequest);
        // Accept either 400 (Netty DecoderResult path) or 431 (RFC 9110 Section 15.5.32)
        assertTrue(response.contains("400") || response.contains("431"),
                "Oversized header should return 400 or 431 but got: " + response);
    }

    /**
     * Verify that a request with headers within the size limit is accepted.
     */
    @Test
    void normalHeaderSizeReturns200() throws Exception {
        String rawRequest = "GET / HTTP/1.1\r\n" +
                "Host: localhost:" + loadBalancerPort + "\r\n" +
                "Accept: */*\r\n" +
                "\r\n";

        String response = sendRawRequest(rawRequest);
        assertTrue(response.contains("200"),
                "Normal-sized headers should return 200 but got: " + response);
    }

    private String sendRawRequest(String rawRequest) throws Exception {
        try (Socket socket = new Socket("127.0.0.1", loadBalancerPort)) {
            socket.setSoTimeout(5000);

            OutputStream out = socket.getOutputStream();
            out.write(rawRequest.getBytes(StandardCharsets.UTF_8));
            out.flush();

            try (BufferedReader reader = new BufferedReader(
                    new InputStreamReader(socket.getInputStream(), StandardCharsets.UTF_8))) {

                String statusLine = reader.readLine();
                if (statusLine == null) {
                    return "";
                }
                return statusLine;
            }
        }
    }
}
