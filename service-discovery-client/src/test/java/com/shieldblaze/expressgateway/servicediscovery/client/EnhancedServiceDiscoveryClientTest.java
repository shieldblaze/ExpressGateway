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
package com.shieldblaze.expressgateway.servicediscovery.client;

import com.shieldblaze.expressgateway.testing.EmbeddedHttpServer;
import io.netty.buffer.Unpooled;
import io.netty.handler.codec.http.DefaultFullHttpResponse;
import io.netty.handler.codec.http.HttpResponseStatus;
import io.netty.handler.codec.http.HttpVersion;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.MethodOrderer;
import org.junit.jupiter.api.Order;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.TestMethodOrder;

import java.io.IOException;
import java.net.http.HttpClient;
import java.nio.charset.StandardCharsets;
import java.time.Duration;
import java.util.List;
import java.util.concurrent.atomic.AtomicInteger;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;

@TestMethodOrder(MethodOrderer.OrderAnnotation.class)
class EnhancedServiceDiscoveryClientTest {

    private static EmbeddedHttpServer server;
    private static ServiceDiscoveryClient client;

    @BeforeAll
    static void setup() throws Exception {
        server = EmbeddedHttpServer.builder()
                .handler(req -> {
                    String uri = req.uri();
                    String body;
                    if (uri.contains("register") || uri.contains("deregister")) {
                        body = "{\"Success\": true}";
                    } else if (uri.contains("get") && uri.contains("id=")) {
                        body = """
                                {
                                  "Success": true,
                                  "Instances": [{
                                    "payload": {
                                      "ID": "svc-1",
                                      "IPAddress": "10.0.0.1",
                                      "Port": 8080,
                                      "TLSEnabled": false
                                    }
                                  }]
                                }
                                """;
                    } else {
                        body = "{\"Success\": false, \"Error\": \"Unknown\"}";
                    }
                    byte[] bytes = body.getBytes(StandardCharsets.UTF_8);
                    DefaultFullHttpResponse resp = new DefaultFullHttpResponse(
                            HttpVersion.HTTP_1_1, HttpResponseStatus.OK, Unpooled.wrappedBuffer(bytes));
                    resp.headers().set("Content-Type", "application/json");
                    resp.headers().setInt("Content-Length", bytes.length);
                    return resp;
                })
                .build()
                .start();

        client = ServiceDiscoveryClient.builder()
                .httpClient(HttpClient.newHttpClient())
                .serverUri(server.baseUrl())
                .retryPolicy(new RetryPolicy(3, Duration.ofMillis(10), Duration.ofMillis(100), Duration.ZERO))
                .circuitBreakerFailureThreshold(5)
                .circuitBreakerResetTimeout(Duration.ofSeconds(1))
                .cacheTtlMillis(5_000)
                .build();
    }

    @AfterAll
    static void tearDown() {
        if (server != null) {
            server.close();
        }
    }

    @Order(1)
    @Test
    void registerService() throws IOException {
        ServiceEntry entry = ServiceEntry.of("svc-1", "10.0.0.1", 8080, false);
        client.registerService(entry);

        // Should be cached after registration
        assertEquals(1, client.cache().size());
    }

    @Order(2)
    @Test
    void lookupServiceFromCache() {
        // Cache was populated by registration
        ServiceEntry entry = client.lookupService("svc-1");
        assertNotNull(entry);
        assertEquals("svc-1", entry.id());
        assertEquals("10.0.0.1", entry.ipAddress());
    }

    @Order(3)
    @Test
    void lookupServiceFromServer() {
        // Clear cache to force server lookup
        client.cache().clear();
        ServiceEntry entry = client.lookupService("svc-1");
        assertNotNull(entry);
        assertEquals("svc-1", entry.id());
    }

    @Order(4)
    @Test
    void deregisterService() throws IOException {
        ServiceEntry entry = ServiceEntry.of("svc-1", "10.0.0.1", 8080, false);
        client.deregisterService(entry);
        // Cache should be cleared for this service
        assertEquals(0, client.cache().size());
    }

    @Order(5)
    @Test
    void circuitBreakerState() {
        assertEquals(CircuitBreaker.State.CLOSED, client.circuitBreaker().state());
    }

    @Test
    void retryWithFailingServer() throws Exception {
        AtomicInteger attempts = new AtomicInteger(0);

        EmbeddedHttpServer failingServer = EmbeddedHttpServer.builder()
                .handler(req -> {
                    int attempt = attempts.incrementAndGet();
                    byte[] bytes;
                    HttpResponseStatus status;
                    if (attempt < 3) {
                        bytes = "error".getBytes(StandardCharsets.UTF_8);
                        status = HttpResponseStatus.INTERNAL_SERVER_ERROR;
                    } else {
                        bytes = "{\"Success\": true}".getBytes(StandardCharsets.UTF_8);
                        status = HttpResponseStatus.OK;
                    }
                    DefaultFullHttpResponse resp = new DefaultFullHttpResponse(
                            HttpVersion.HTTP_1_1, status, Unpooled.wrappedBuffer(bytes));
                    resp.headers().set("Content-Type", "application/json");
                    resp.headers().setInt("Content-Length", bytes.length);
                    return resp;
                })
                .build()
                .start();

        try {
            ServiceDiscoveryClient retryClient = ServiceDiscoveryClient.builder()
                    .httpClient(HttpClient.newHttpClient())
                    .serverUri(failingServer.baseUrl())
                    .retryPolicy(new RetryPolicy(3, Duration.ofMillis(10), Duration.ofMillis(100), Duration.ZERO))
                    .circuitBreakerFailureThreshold(10)
                    .circuitBreakerResetTimeout(Duration.ofSeconds(30))
                    .cacheTtlMillis(5_000)
                    .build();

            ServiceEntry entry = ServiceEntry.of("retry-svc", "10.0.0.1", 8080, false);
            retryClient.registerService(entry);
            assertEquals(3, attempts.get(), "Should succeed on 3rd attempt after 2 retries");
        } finally {
            failingServer.close();
        }
    }

    @Test
    void failoverBetweenServers() throws Exception {
        // First server always fails
        EmbeddedHttpServer badServer = EmbeddedHttpServer.builder()
                .responseBody("error")
                .responseStatus(HttpResponseStatus.INTERNAL_SERVER_ERROR)
                .build()
                .start();

        // Second server always succeeds
        EmbeddedHttpServer goodServer = EmbeddedHttpServer.builder()
                .handler(req -> {
                    byte[] bytes = "{\"Success\": true}".getBytes(StandardCharsets.UTF_8);
                    DefaultFullHttpResponse resp = new DefaultFullHttpResponse(
                            HttpVersion.HTTP_1_1, HttpResponseStatus.OK, Unpooled.wrappedBuffer(bytes));
                    resp.headers().set("Content-Type", "application/json");
                    resp.headers().setInt("Content-Length", bytes.length);
                    return resp;
                })
                .build()
                .start();

        try {
            ServiceDiscoveryClient failoverClient = ServiceDiscoveryClient.builder()
                    .httpClient(HttpClient.newHttpClient())
                    .serverUris(List.of(badServer.baseUrl(), goodServer.baseUrl()))
                    .unhealthyThreshold(1)
                    .retryPolicy(new RetryPolicy(3, Duration.ofMillis(5), Duration.ofMillis(50), Duration.ZERO))
                    .circuitBreakerFailureThreshold(10)
                    .circuitBreakerResetTimeout(Duration.ofSeconds(30))
                    .cacheTtlMillis(5_000)
                    .build();

            ServiceEntry entry = ServiceEntry.of("failover-svc", "10.0.0.1", 8080, false);
            failoverClient.registerService(entry);
            // Should succeed via the good server
            assertEquals(1, failoverClient.cache().size());
        } finally {
            badServer.close();
            goodServer.close();
        }
    }

    @Test
    void circuitBreakerBlocksRequests() {
        ServiceDiscoveryClient cbClient = ServiceDiscoveryClient.builder()
                .httpClient(HttpClient.newHttpClient())
                .serverUri("http://127.0.0.1:1") // Port 1 on localhost: connection refused fast
                .circuitBreakerFailureThreshold(1)
                .circuitBreakerResetTimeout(Duration.ofMinutes(10))
                .retryPolicy(RetryPolicy.NONE)
                .cacheTtlMillis(5_000)
                .build();

        // First call will fail (connection refused/timeout) and open the circuit
        ServiceEntry entry = ServiceEntry.of("cb-svc", "10.0.0.1", 8080, false);
        assertThrows(IOException.class, () -> cbClient.registerService(entry));

        // Circuit should now be open
        assertEquals(CircuitBreaker.State.OPEN, cbClient.circuitBreaker().state());

        // Subsequent calls should be fast-rejected
        assertThrows(CircuitBreakerOpenException.class,
                () -> cbClient.registerService(entry));
    }

    @Test
    void lookupNonexistentServiceReturnsNull() {
        ServiceDiscoveryClient emptyClient = ServiceDiscoveryClient.builder()
                .httpClient(HttpClient.newHttpClient())
                .serverUri("http://127.0.0.1:1")
                .retryPolicy(RetryPolicy.NONE)
                .circuitBreakerFailureThreshold(100)
                .circuitBreakerResetTimeout(Duration.ofSeconds(1))
                .cacheTtlMillis(5_000)
                .build();

        ServiceEntry result = emptyClient.lookupService("nonexistent");
        assertNull(result);
    }

    @Test
    void builderRequiresServerUri() {
        assertThrows(IllegalStateException.class, () -> ServiceDiscoveryClient.builder()
                .cacheTtlMillis(5_000)
                .build());
    }
}
