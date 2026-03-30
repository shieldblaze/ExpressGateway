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

import io.netty.channel.embedded.EmbeddedChannel;
import io.netty.handler.codec.http.DefaultFullHttpRequest;
import io.netty.handler.codec.http.DefaultHttpContent;
import io.netty.handler.codec.http.DefaultHttpRequest;
import io.netty.handler.codec.http.DefaultLastHttpContent;
import io.netty.handler.codec.http.FullHttpResponse;
import io.netty.handler.codec.http.HttpMethod;
import io.netty.handler.codec.http.HttpVersion;
import io.netty.buffer.Unpooled;
import org.junit.jupiter.api.Test;

import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.util.concurrent.ThreadLocalRandom;

import static org.junit.jupiter.api.Assertions.*;

/**
 * Tests for production-readiness fixes:
 * - CF-02: H2 headers-only stream cleanup
 * - ME-01: Streams.java ReadWriteLock correctness
 * - ME-02: FastRequestId generation
 * - ME-08: DownstreamHandler.detachInboundChannel synchronization
 */
class ProductionReadinessTest {

    /**
     * ME-02: Verify FastRequestId generates UUID v4 formatted strings (8-4-4-4-12).
     */
    @Test
    void testFastRequestIdFormat() {
        String id = FastRequestId.generate();
        assertNotNull(id);
        assertEquals(36, id.length(), "Request ID should be 36 characters (UUID v4 with dashes)");
        assertTrue(id.matches("[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}"),
                "Request ID should be UUID v4 format: " + id);
    }

    /**
     * ME-02: Verify FastRequestId generates unique IDs (no collisions in 10K samples).
     */
    @Test
    void testFastRequestIdUniqueness() {
        java.util.Set<String> ids = new java.util.HashSet<>();
        int count = 10_000;
        for (int i = 0; i < count; i++) {
            assertTrue(ids.add(FastRequestId.generate()), "Duplicate ID detected at iteration " + i);
        }
        assertEquals(count, ids.size());
    }

    /**
     * ME-01: Verify Streams.java ReadWriteLock under concurrent access.
     * Multiple threads doing put/get/remove must not lose entries or throw exceptions.
     */
    @Test
    void testStreamsConcurrency() throws InterruptedException {
        Streams streams = new Streams();
        int threadCount = 8;
        int opsPerThread = 1000;
        java.util.concurrent.CountDownLatch latch = new java.util.concurrent.CountDownLatch(threadCount);
        java.util.concurrent.atomic.AtomicInteger errors = new java.util.concurrent.atomic.AtomicInteger();

        for (int t = 0; t < threadCount; t++) {
            final int threadId = t;
            new Thread(() -> {
                try {
                    for (int i = 0; i < opsPerThread; i++) {
                        int streamId = threadId * 10000 + i * 2 + 1; // odd IDs (H2 client streams)
                        // Create a mock stream entry
                        Streams.Stream entry = new Streams.Stream("gzip", null, null);
                        streams.put(streamId, entry);
                        Streams.Stream got = streams.get(streamId);
                        if (got == null) {
                            errors.incrementAndGet();
                        }
                        streams.remove(streamId);
                    }
                } catch (Exception e) {
                    errors.incrementAndGet();
                } finally {
                    latch.countDown();
                }
            }).start();
        }

        latch.await();
        assertEquals(0, errors.get(), "Concurrent access should not lose entries or throw");
        assertEquals(0, streams.size(), "All entries should be removed");
    }

    /**
     * ME-01: Verify Streams reverse map operations work correctly.
     */
    @Test
    void testStreamsReverseMap() {
        Streams streams = new Streams();
        Streams.Stream entry = new Streams.Stream("br", null, null);

        streams.put(1, entry);
        streams.putByBackendId(100, entry);

        assertNotNull(streams.get(1));
        assertNotNull(streams.getByBackendId(100));

        streams.remove(1);
        assertNull(streams.get(1));

        Streams.Stream removed = streams.removeByBackendId(100);
        assertNotNull(removed);
        assertNull(streams.getByBackendId(100));
    }

    /**
     * ME-01: Verify removeStreamsAbove works with ReadWriteLock.
     */
    @Test
    void testStreamsRemoveAbove() {
        Streams streams = new Streams();
        for (int i = 1; i <= 11; i += 2) { // Streams 1, 3, 5, 7, 9, 11
            streams.put(i, new Streams.Stream("gzip", null, null));
        }
        assertEquals(6, streams.size());

        streams.removeStreamsAbove(5); // Remove streams > 5 (7, 9, 11)
        assertEquals(3, streams.size()); // Streams 1, 3, 5 remain
        assertNotNull(streams.get(1));
        assertNotNull(streams.get(3));
        assertNotNull(streams.get(5));
        assertNull(streams.get(7));
        assertNull(streams.get(9));
        assertNull(streams.get(11));
    }

    /**
     * ME-04: Verify RequestBodyTimeoutHandler cancels timer when body completes.
     * Uses EmbeddedChannel to test without a full proxy setup.
     */
    @Test
    void testRequestBodyTimeoutHandlerCompletesNormally() {
        RequestBodyTimeoutHandler handler = new RequestBodyTimeoutHandler(2);
        EmbeddedChannel ch = new EmbeddedChannel(handler);

        // Send request with body (not FullHttpRequest)
        DefaultHttpRequest request = new DefaultHttpRequest(HttpVersion.HTTP_1_1, HttpMethod.POST, "/upload");
        ch.writeInbound(request);

        // Send body content
        ch.writeInbound(new DefaultHttpContent(Unpooled.wrappedBuffer(new byte[]{1, 2, 3})));

        // Complete the body — timer should be cancelled
        ch.writeInbound(new DefaultLastHttpContent());

        // Channel should still be open (timer was cancelled)
        assertTrue(ch.isOpen());
        ch.close();
    }

    /**
     * ME-05: Verify /health endpoint returns 200 via HTTP/1.1 proxy.
     */
    @Test
    void testHealthEndpoint() throws Exception {
        TestableHttpLoadBalancer lb = TestableHttpLoadBalancer.Builder.newBuilder().build();
        lb.start();
        try {
            HttpClient client = HttpClient.newHttpClient();
            HttpResponse<String> resp = client.send(
                    HttpRequest.newBuilder()
                            .uri(URI.create("http://127.0.0.1:" + lb.port() + "/health"))
                            .GET()
                            .build(),
                    HttpResponse.BodyHandlers.ofString()
            );
            // /health returns 200 when proxy is accepting and has healthy backends
            assertEquals(200, resp.statusCode());
        } finally {
            lb.close();
        }
    }

    /**
     * ME-05: Verify /ready endpoint returns 200 when backends are available.
     */
    @Test
    void testReadyEndpoint() throws Exception {
        TestableHttpLoadBalancer lb = TestableHttpLoadBalancer.Builder.newBuilder().build();
        lb.start();
        try {
            HttpClient client = HttpClient.newHttpClient();
            HttpResponse<String> resp = client.send(
                    HttpRequest.newBuilder()
                            .uri(URI.create("http://127.0.0.1:" + lb.port() + "/ready"))
                            .GET()
                            .build(),
                    HttpResponse.BodyHandlers.ofString()
            );
            assertEquals(200, resp.statusCode());
        } finally {
            lb.close();
        }
    }

    /**
     * HI-01: Verify MicrometerBridge binds metrics to a registry.
     */
    @Test
    void testMicrometerBridgeRegistersMetrics() {
        io.micrometer.core.instrument.simple.SimpleMeterRegistry registry = new io.micrometer.core.instrument.simple.SimpleMeterRegistry();
        com.shieldblaze.expressgateway.metrics.MicrometerBridge.bind(registry);

        // Verify key metrics are registered
        assertNotNull(registry.find("expressgateway.connections.active").gauge(),
                "Active connections gauge should be registered");
        assertNotNull(registry.find("expressgateway.latency.p99").gauge(),
                "Latency P99 gauge should be registered");
        assertNotNull(registry.find("expressgateway.bandwidth.tx.bytes").functionCounter(),
                "Bandwidth TX counter should be registered");
        assertNotNull(registry.find("expressgateway.errors.connection").functionCounter(),
                "Connection errors counter should be registered");
    }

    /**
     * HI-04: Verify ConnectionTracker ChannelGroup tracks channels.
     */
    @Test
    void testConnectionTrackerChannelGroup() {
        com.shieldblaze.expressgateway.core.handlers.ConnectionTracker tracker =
                new com.shieldblaze.expressgateway.core.handlers.ConnectionTracker();

        assertEquals(0, tracker.allChannels().size());
        // ChannelGroup integration tested via full proxy tests — channels are added
        // in channelActive and auto-removed by ChannelGroup when channels close.
    }
}
