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
package com.shieldblaze.expressgateway.protocol.http.http3;

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.NodeBuilder;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterBuilder;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.HTTPRoundRobin;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.sessionpersistence.NOOPSessionPersistence;
import com.shieldblaze.expressgateway.configuration.http3.Http3Configuration;
import com.shieldblaze.expressgateway.configuration.quic.QuicConfiguration;
import com.shieldblaze.expressgateway.protocol.quic.QuicConnectionPool;
import io.netty.channel.embedded.EmbeddedChannel;
import io.netty.handler.codec.http.DefaultFullHttpResponse;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpResponse;
import io.netty.handler.codec.http.HttpResponseStatus;
import io.netty.handler.codec.http.HttpVersion;
import io.netty.handler.codec.http2.DefaultHttp2Headers;
import io.netty.handler.codec.http2.DefaultHttp2HeadersFrame;
import io.netty.handler.codec.http2.Http2Headers;
import io.netty.handler.codec.http2.Http2HeadersFrame;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Nested;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.Timeout;

import java.lang.reflect.Constructor;
import java.net.InetSocketAddress;

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Unit tests for HTTP/3 components: configuration validation, connection pool lifecycle,
 * connection stream tracking, and Alt-Svc header injection.
 */
@Timeout(value = 30)
class Http3LifecycleTest {

    // ── Factory helpers ──────────────────────────────────────────────────────

    private static Http3Configuration newMutableH3Config() {
        try {
            Constructor<Http3Configuration> ctor = Http3Configuration.class.getDeclaredConstructor();
            ctor.setAccessible(true);
            return ctor.newInstance();
        } catch (Exception e) {
            throw new RuntimeException("Failed to create Http3Configuration via reflection", e);
        }
    }

    private static QuicConfiguration newMutableQuicConfig() {
        try {
            Constructor<QuicConfiguration> ctor = QuicConfiguration.class.getDeclaredConstructor();
            ctor.setAccessible(true);
            return ctor.newInstance();
        } catch (Exception e) {
            throw new RuntimeException("Failed to create QuicConfiguration via reflection", e);
        }
    }

    private static QuicConfiguration buildValidatedQuicConfig(int maxConnsPerNode, long maxStreamsBidi,
                                                                long maxIdleTimeoutMs) {
        QuicConfiguration config = newMutableQuicConfig();
        config.setEnabled(true)
                .setMaxIdleTimeoutMs(maxIdleTimeoutMs)
                .setInitialMaxData(10_000_000)
                .setInitialMaxStreamDataBidiLocal(1_000_000)
                .setInitialMaxStreamDataBidiRemote(1_000_000)
                .setInitialMaxStreamDataUni(1_000_000)
                .setInitialMaxStreamsBidi(maxStreamsBidi)
                .setInitialMaxStreamsUni(3)
                .setMaxConnectionsPerNode(maxConnsPerNode)
                .setPort(443)
                .setZeroRttEnabled(false)
                .setGracefulShutdownDrainMs(5000);
        config.validate();
        return config;
    }

    // ====================================================================
    // 1. Http3Configuration Tests (HTTP/3-specific settings only)
    // ====================================================================

    @Nested
    @DisplayName("Http3Configuration")
    class Http3ConfigurationTests {

        @Test
        @DisplayName("DEFAULT instance has correct defaults and is validated")
        void defaultInstanceHasCorrectDefaults() {
            Http3Configuration config = Http3Configuration.DEFAULT;
            assertTrue(config.validated(), "DEFAULT must be pre-validated");

            assertEquals(0L, config.altSvcMaxAge(),
                    "Default Alt-Svc max-age must be 0 (disabled)");
            assertEquals(0L, config.qpackMaxTableCapacity(),
                    "Default QPACK max table capacity must be 0");
            assertEquals(0L, config.qpackBlockedStreams(),
                    "Default QPACK blocked streams must be 0");
        }

        @Test
        @DisplayName("valid configuration validates successfully")
        void validConfigurationValidates() {
            Http3Configuration config = newMutableH3Config();
            config.setAltSvcMaxAge(7200)
                    .setQpackMaxTableCapacity(4096)
                    .setQpackBlockedStreams(100);

            assertFalse(config.validated());
            assertDoesNotThrow(config::validate);
            assertTrue(config.validated());
        }

        @Test
        @DisplayName("accessing getter on unvalidated config throws IllegalStateException")
        void unvalidatedConfigThrowsOnAccess() {
            Http3Configuration config = newMutableH3Config();
            assertThrows(IllegalStateException.class, config::altSvcMaxAge);
            assertThrows(IllegalStateException.class, config::qpackMaxTableCapacity);
            assertThrows(IllegalStateException.class, config::qpackBlockedStreams);
        }

        @Test
        @DisplayName("validation rejects negative altSvcMaxAge")
        void validationRejectsNegativeAltSvcMaxAge() {
            Http3Configuration config = newMutableH3Config();
            config.setAltSvcMaxAge(-1).setQpackMaxTableCapacity(0).setQpackBlockedStreams(0);
            assertThrows(IllegalArgumentException.class, config::validate);
        }

        @Test
        @DisplayName("validation rejects negative qpackMaxTableCapacity")
        void validationRejectsNegativeQpackMaxTableCapacity() {
            Http3Configuration config = newMutableH3Config();
            config.setAltSvcMaxAge(0).setQpackMaxTableCapacity(-1).setQpackBlockedStreams(0);
            assertThrows(IllegalArgumentException.class, config::validate);
        }

        @Test
        @DisplayName("validation rejects negative qpackBlockedStreams")
        void validationRejectsNegativeQpackBlockedStreams() {
            Http3Configuration config = newMutableH3Config();
            config.setAltSvcMaxAge(0).setQpackMaxTableCapacity(0).setQpackBlockedStreams(-1);
            assertThrows(IllegalArgumentException.class, config::validate);
        }

        @Test
        @DisplayName("validation accepts zero altSvcMaxAge")
        void validationAcceptsZeroAltSvcMaxAge() {
            Http3Configuration config = newMutableH3Config();
            config.setAltSvcMaxAge(0).setQpackMaxTableCapacity(0).setQpackBlockedStreams(0);
            assertDoesNotThrow(config::validate);
        }
    }

    // ====================================================================
    // 2. QuicConfiguration Tests (QUIC transport parameters)
    // ====================================================================

    @Nested
    @DisplayName("QuicConfiguration")
    class QuicConfigurationTests {

        @Test
        @DisplayName("DEFAULT instance has spec-compliant defaults")
        void defaultInstanceHasCorrectDefaults() {
            QuicConfiguration config = QuicConfiguration.DEFAULT;
            assertTrue(config.validated());
            assertTrue(config.enabled());
            assertEquals(30_000L, config.maxIdleTimeoutMs());
            assertEquals(10_000_000L, config.initialMaxData());
            assertEquals(1_000_000L, config.initialMaxStreamDataBidiLocal());
            assertEquals(1_000_000L, config.initialMaxStreamDataBidiRemote());
            assertEquals(1_000_000L, config.initialMaxStreamDataUni());
            assertEquals(100L, config.initialMaxStreamsBidi());
            assertEquals(3L, config.initialMaxStreamsUni());
            assertEquals(4, config.maxConnectionsPerNode());
            assertEquals(443, config.port());
            assertFalse(config.zeroRttEnabled());
            assertEquals(5000L, config.gracefulShutdownDrainMs());
        }

        @Test
        @DisplayName("validation rejects zero initialMaxData")
        void validationRejectsZeroInitialMaxData() {
            QuicConfiguration config = newMutableQuicConfig();
            config.setEnabled(true).setMaxIdleTimeoutMs(30_000).setInitialMaxData(0)
                    .setInitialMaxStreamDataBidiLocal(1_000_000).setInitialMaxStreamDataBidiRemote(1_000_000)
                    .setInitialMaxStreamDataUni(1_000_000).setInitialMaxStreamsBidi(100)
                    .setInitialMaxStreamsUni(3).setMaxConnectionsPerNode(4).setPort(443)
                    .setGracefulShutdownDrainMs(5000);
            assertThrows(IllegalArgumentException.class, config::validate);
        }

        @Test
        @DisplayName("validation rejects zero port")
        void validationRejectsZeroPort() {
            QuicConfiguration config = newMutableQuicConfig();
            config.setEnabled(true).setMaxIdleTimeoutMs(30_000).setInitialMaxData(10_000_000)
                    .setInitialMaxStreamDataBidiLocal(1_000_000).setInitialMaxStreamDataBidiRemote(1_000_000)
                    .setInitialMaxStreamDataUni(1_000_000).setInitialMaxStreamsBidi(100)
                    .setInitialMaxStreamsUni(3).setMaxConnectionsPerNode(4).setPort(0)
                    .setGracefulShutdownDrainMs(5000);
            assertThrows(IllegalArgumentException.class, config::validate);
        }

        @Test
        @DisplayName("validation accepts zero maxIdleTimeoutMs")
        void validationAcceptsZeroMaxIdleTimeout() {
            QuicConfiguration config = newMutableQuicConfig();
            config.setEnabled(true).setMaxIdleTimeoutMs(0).setInitialMaxData(10_000_000)
                    .setInitialMaxStreamDataBidiLocal(1_000_000).setInitialMaxStreamDataBidiRemote(1_000_000)
                    .setInitialMaxStreamDataUni(1_000_000).setInitialMaxStreamsBidi(100)
                    .setInitialMaxStreamsUni(3).setMaxConnectionsPerNode(4).setPort(443)
                    .setGracefulShutdownDrainMs(5000);
            assertDoesNotThrow(config::validate);
        }
    }

    // ====================================================================
    // 3. Http3Connection Tests
    // ====================================================================

    @Nested
    @DisplayName("Http3Connection")
    class Http3ConnectionTests {

        private Cluster cluster;
        private Node node;

        @BeforeEach
        void setUp() throws Exception {
            cluster = ClusterBuilder.newBuilder()
                    .withLoadBalance(new HTTPRoundRobin(NOOPSessionPersistence.INSTANCE))
                    .build();

            node = NodeBuilder.newBuilder()
                    .withCluster(cluster)
                    .withSocketAddress(new InetSocketAddress("127.0.0.1", 8080))
                    .build();
        }

        @AfterEach
        void tearDown() {
            if (cluster != null) {
                cluster.close();
            }
        }

        @Test
        @DisplayName("stream counting increments and decrements correctly")
        void streamCountingIsCorrect() {
            Http3Connection conn = new Http3Connection(node);

            assertEquals(0, conn.activeStreams());
            assertEquals(1, conn.incrementActiveStreams());
            assertEquals(2, conn.incrementActiveStreams());
            assertEquals(3, conn.incrementActiveStreams());
            assertEquals(4, conn.incrementActiveStreams());
            assertEquals(5, conn.incrementActiveStreams());
            assertEquals(5, conn.activeStreams());

            assertEquals(4, conn.decrementActiveStreams());
            assertEquals(3, conn.decrementActiveStreams());
            assertEquals(2, conn.decrementActiveStreams());
            assertEquals(2, conn.activeStreams());
        }

        @Test
        @DisplayName("idle tracking sets timestamp when streams reach zero")
        void idleTrackingSetOnZeroStreams() {
            Http3Connection conn = new Http3Connection(node);
            assertEquals(0L, conn.idleSinceNanos());

            conn.incrementActiveStreams();
            assertEquals(0L, conn.idleSinceNanos());

            conn.decrementActiveStreams();
            long idleSince = conn.idleSinceNanos();
            assertTrue(idleSince > 0);

            conn.incrementActiveStreams();
            assertEquals(0L, conn.idleSinceNanos());
        }

        @Test
        @DisplayName("hasStreamCapacity returns false when at limit")
        void hasStreamCapacityEnforcesLimit() {
            Http3Connection conn = new Http3Connection(node);
            int maxStreams = 3;

            assertTrue(conn.hasStreamCapacity(maxStreams));
            conn.incrementActiveStreams();
            assertTrue(conn.hasStreamCapacity(maxStreams));
            conn.incrementActiveStreams();
            assertTrue(conn.hasStreamCapacity(maxStreams));
            conn.incrementActiveStreams();
            assertFalse(conn.hasStreamCapacity(maxStreams));

            conn.decrementActiveStreams();
            assertTrue(conn.hasStreamCapacity(maxStreams));
        }

        @Test
        @DisplayName("isHttp3 returns true")
        void isHttp3ReturnsTrue() {
            Http3Connection conn = new Http3Connection(node);
            assertTrue(conn.isHttp3());
        }

        @Test
        @DisplayName("initial state is INITIALIZED")
        void initialStateIsInitialized() {
            Http3Connection conn = new Http3Connection(node);
            assertEquals(com.shieldblaze.expressgateway.backend.Connection.State.INITIALIZED, conn.state());
        }

        @Test
        @DisplayName("node reference is stored correctly")
        void nodeReferenceIsCorrect() {
            Http3Connection conn = new Http3Connection(node);
            assertEquals(node, conn.node());
        }

        @Test
        @DisplayName("quicChannel is null before assignment")
        void quicChannelIsNullBeforeAssignment() {
            Http3Connection conn = new Http3Connection(node);
            assertNull(conn.quicChannel());
        }

        @Test
        @DisplayName("toString includes activeStreams")
        void toStringIncludesActiveStreams() {
            Http3Connection conn = new Http3Connection(node);
            conn.incrementActiveStreams();
            conn.incrementActiveStreams();
            String str = conn.toString();
            assertTrue(str.contains("activeStreams=2"), "got: " + str);
        }

        @Test
        @DisplayName("idle timestamp is monotonically increasing across transitions")
        void idleTimestampIsMonotonic() throws InterruptedException {
            Http3Connection conn = new Http3Connection(node);
            conn.incrementActiveStreams();
            conn.decrementActiveStreams();
            long firstIdle = conn.idleSinceNanos();
            assertTrue(firstIdle > 0);

            Thread.sleep(1);

            conn.incrementActiveStreams();
            conn.decrementActiveStreams();
            long secondIdle = conn.idleSinceNanos();
            assertTrue(secondIdle >= firstIdle);
        }
    }

    // ====================================================================
    // 4. QuicConnectionPool Tests (with Http3Connection)
    // ====================================================================

    @Nested
    @DisplayName("QuicConnectionPool")
    class ConnectionPoolTests {

        private Cluster cluster;
        private Node node;

        @BeforeEach
        void setUp() throws Exception {
            cluster = ClusterBuilder.newBuilder()
                    .withLoadBalance(new HTTPRoundRobin(NOOPSessionPersistence.INSTANCE))
                    .build();

            node = NodeBuilder.newBuilder()
                    .withCluster(cluster)
                    .withSocketAddress(new InetSocketAddress("127.0.0.1", 9090))
                    .build();
        }

        @AfterEach
        void tearDown() {
            if (cluster != null) {
                cluster.close();
            }
        }

        @Test
        @DisplayName("acquire returns null for unregistered node")
        void acquireReturnsNullForUnregisteredNode() {
            QuicConfiguration config = buildValidatedQuicConfig(4, 100, 30_000);
            QuicConnectionPool<Http3Connection> pool = new QuicConnectionPool<>(config);
            try {
                assertNull(pool.acquire(node));
            } finally {
                pool.closeAll();
            }
        }

        @Test
        @DisplayName("canCreateConnection enforces per-node limit")
        void canCreateConnectionEnforcesLimit() {
            int maxPerNode = 2;
            QuicConfiguration config = buildValidatedQuicConfig(maxPerNode, 100, 30_000);
            QuicConnectionPool<Http3Connection> pool = new QuicConnectionPool<>(config);
            try {
                assertTrue(pool.canCreateConnection(node));

                Http3Connection conn1 = new Http3Connection(node);
                pool.register(node, conn1);
                assertTrue(pool.canCreateConnection(node));

                Http3Connection conn2 = new Http3Connection(node);
                pool.register(node, conn2);
                assertFalse(pool.canCreateConnection(node));
            } finally {
                pool.closeAll();
            }
        }

        @Test
        @DisplayName("acquire returns INITIALIZED connection (pre-handshake)")
        void acquireReturnsInitializedConnection() {
            QuicConfiguration config = buildValidatedQuicConfig(4, 100, 30_000);
            QuicConnectionPool<Http3Connection> pool = new QuicConnectionPool<>(config);
            try {
                Http3Connection conn = new Http3Connection(node);
                pool.register(node, conn);

                Http3Connection acquired = pool.acquire(node);
                assertNotNull(acquired);
                assertEquals(conn, acquired);
            } finally {
                pool.closeAll();
            }
        }

        @Test
        @DisplayName("acquire selects least-loaded connection")
        void acquireSelectsLeastLoaded() {
            QuicConfiguration config = buildValidatedQuicConfig(4, 100, 30_000);
            QuicConnectionPool<Http3Connection> pool = new QuicConnectionPool<>(config);
            try {
                Http3Connection connHeavy = new Http3Connection(node);
                Http3Connection connLight = new Http3Connection(node);
                pool.register(node, connHeavy);
                pool.register(node, connLight);

                for (int i = 0; i < 10; i++) {
                    connHeavy.incrementActiveStreams();
                }
                connLight.incrementActiveStreams();
                connLight.incrementActiveStreams();

                Http3Connection acquired = pool.acquire(node);
                assertNotNull(acquired);
                assertEquals(connLight, acquired);
            } finally {
                pool.closeAll();
            }
        }

        @Test
        @DisplayName("acquire returns null when all connections at capacity")
        void acquireReturnsNullWhenAllAtCapacity() {
            int maxStreams = 2;
            QuicConfiguration config = buildValidatedQuicConfig(4, maxStreams, 30_000);
            QuicConnectionPool<Http3Connection> pool = new QuicConnectionPool<>(config);
            try {
                Http3Connection conn = new Http3Connection(node);
                pool.register(node, conn);

                for (int i = 0; i < maxStreams; i++) {
                    conn.incrementActiveStreams();
                }

                assertNull(pool.acquire(node));
            } finally {
                pool.closeAll();
            }
        }

        @Test
        @DisplayName("evict removes connection from pool")
        void evictRemovesConnection() {
            QuicConfiguration config = buildValidatedQuicConfig(4, 100, 30_000);
            QuicConnectionPool<Http3Connection> pool = new QuicConnectionPool<>(config);
            try {
                Http3Connection conn = new Http3Connection(node);
                pool.register(node, conn);
                assertNotNull(pool.acquire(node));

                pool.evict(conn);
                assertNull(pool.acquire(node));
            } finally {
                pool.closeAll();
            }
        }

        @Test
        @DisplayName("closeAll empties pool")
        void closeAllEmptiesPool() {
            QuicConfiguration config = buildValidatedQuicConfig(4, 100, 30_000);
            QuicConnectionPool<Http3Connection> pool = new QuicConnectionPool<>(config);

            Http3Connection conn1 = new Http3Connection(node);
            Http3Connection conn2 = new Http3Connection(node);
            pool.register(node, conn1);
            pool.register(node, conn2);

            pool.closeAll();

            assertNull(pool.acquire(node));
            assertTrue(pool.allActiveConnections().isEmpty());
        }

        @Test
        @DisplayName("allActiveConnections returns all registered connections")
        void allActiveConnectionsReturnsAll() throws Exception {
            QuicConfiguration config = buildValidatedQuicConfig(4, 100, 30_000);
            QuicConnectionPool<Http3Connection> pool = new QuicConnectionPool<>(config);
            try {
                Node node2 = NodeBuilder.newBuilder()
                        .withCluster(cluster)
                        .withSocketAddress(new InetSocketAddress("127.0.0.1", 9091))
                        .build();

                Http3Connection conn1 = new Http3Connection(node);
                Http3Connection conn2 = new Http3Connection(node2);
                pool.register(node, conn1);
                pool.register(node2, conn2);

                assertEquals(2, pool.allActiveConnections().size());
            } finally {
                pool.closeAll();
            }
        }

        @Test
        @DisplayName("releaseStream does not remove connection from pool")
        void releaseStreamKeepsConnectionInPool() {
            QuicConfiguration config = buildValidatedQuicConfig(4, 100, 30_000);
            QuicConnectionPool<Http3Connection> pool = new QuicConnectionPool<>(config);
            try {
                Http3Connection conn = new Http3Connection(node);
                pool.register(node, conn);
                conn.incrementActiveStreams();

                conn.decrementActiveStreams();
                pool.releaseStream(node, conn);

                Http3Connection acquired = pool.acquire(node);
                assertNotNull(acquired);
                assertEquals(conn, acquired);
            } finally {
                pool.closeAll();
            }
        }

        @Test
        @DisplayName("allBackendsWritable returns true when pool is empty")
        void allBackendsWritableWhenEmpty() {
            QuicConfiguration config = buildValidatedQuicConfig(4, 100, 30_000);
            QuicConnectionPool<Http3Connection> pool = new QuicConnectionPool<>(config);
            try {
                assertTrue(pool.allBackendsWritable());
            } finally {
                pool.closeAll();
            }
        }

        @Test
        @DisplayName("zero maxIdleTimeoutMs disables eviction executor")
        void zeroIdleTimeoutDisablesEviction() {
            QuicConfiguration config = buildValidatedQuicConfig(4, 100, 0);
            QuicConnectionPool<Http3Connection> pool = new QuicConnectionPool<>(config);
            try {
                Http3Connection conn = new Http3Connection(node);
                pool.register(node, conn);
                assertNotNull(pool.acquire(node));
            } finally {
                pool.closeAll();
            }
        }

        @Test
        @DisplayName("acquire prefers idle connection over active one")
        void acquirePrefersIdleConnection() {
            QuicConfiguration config = buildValidatedQuicConfig(4, 100, 30_000);
            QuicConnectionPool<Http3Connection> pool = new QuicConnectionPool<>(config);
            try {
                Http3Connection busy = new Http3Connection(node);
                Http3Connection idle = new Http3Connection(node);
                pool.register(node, busy);
                pool.register(node, idle);

                busy.incrementActiveStreams();

                Http3Connection acquired = pool.acquire(node);
                assertNotNull(acquired);
                assertEquals(idle, acquired);
            } finally {
                pool.closeAll();
            }
        }
    }

    // ====================================================================
    // 5. AltSvcHandler Tests
    // ====================================================================

    @Nested
    @DisplayName("AltSvcHandler")
    class AltSvcHandlerTests {

        @Test
        @DisplayName("injects Alt-Svc header into HTTP/1.1 response")
        void injectsAltSvcIntoHttp11Response() {
            AltSvcHandler handler = new AltSvcHandler(443, 3600);
            EmbeddedChannel channel = new EmbeddedChannel(handler);

            DefaultFullHttpResponse response = new DefaultFullHttpResponse(
                    HttpVersion.HTTP_1_1, HttpResponseStatus.OK);
            response.headers().set(HttpHeaderNames.CONTENT_LENGTH, 0);

            assertTrue(channel.writeOutbound(response));

            HttpResponse outbound = channel.readOutbound();
            assertNotNull(outbound);

            String altSvc = outbound.headers().get("alt-svc");
            assertNotNull(altSvc);
            assertEquals("h3=\":443\"; ma=3600", altSvc);

            channel.close();
        }

        @Test
        @DisplayName("injects Alt-Svc header into HTTP/2 response headers frame")
        void injectsAltSvcIntoHttp2ResponseHeaders() {
            AltSvcHandler handler = new AltSvcHandler(8443, 7200);
            EmbeddedChannel channel = new EmbeddedChannel(handler);

            Http2Headers headers = new DefaultHttp2Headers();
            headers.status("200");
            headers.set("content-type", "text/plain");
            Http2HeadersFrame headersFrame = new DefaultHttp2HeadersFrame(headers, true);

            assertTrue(channel.writeOutbound(headersFrame));

            Http2HeadersFrame outbound = channel.readOutbound();
            assertNotNull(outbound);

            CharSequence altSvc = outbound.headers().get("alt-svc");
            assertNotNull(altSvc);
            assertEquals("h3=\":8443\"; ma=7200", altSvc.toString());

            channel.close();
        }

        @Test
        @DisplayName("does NOT inject Alt-Svc into HTTP/2 trailer frame")
        void doesNotInjectAltSvcIntoTrailers() {
            AltSvcHandler handler = new AltSvcHandler(443, 3600);
            EmbeddedChannel channel = new EmbeddedChannel(handler);

            Http2Headers trailers = new DefaultHttp2Headers();
            trailers.set("grpc-status", "0");
            Http2HeadersFrame trailerFrame = new DefaultHttp2HeadersFrame(trailers, true);

            assertTrue(channel.writeOutbound(trailerFrame));

            Http2HeadersFrame outbound = channel.readOutbound();
            assertNotNull(outbound);
            assertNull(outbound.headers().get("alt-svc"));

            channel.close();
        }

        @Test
        @DisplayName("does NOT overwrite existing Alt-Svc")
        void doesNotOverwriteExistingAltSvc() {
            AltSvcHandler handler = new AltSvcHandler(443, 3600);
            EmbeddedChannel channel = new EmbeddedChannel(handler);

            DefaultFullHttpResponse response = new DefaultFullHttpResponse(
                    HttpVersion.HTTP_1_1, HttpResponseStatus.OK);
            response.headers().set("alt-svc", "h3=\":9443\"; ma=1800, h2=\":443\"; ma=600");
            response.headers().set(HttpHeaderNames.CONTENT_LENGTH, 0);

            channel.writeOutbound(response);

            HttpResponse outbound = channel.readOutbound();
            assertEquals("h3=\":9443\"; ma=1800, h2=\":443\"; ma=600",
                    outbound.headers().get("alt-svc"));

            channel.close();
        }

        @Test
        @DisplayName("constructor rejects invalid port 0")
        void constructorRejectsPortZero() {
            assertThrows(IllegalArgumentException.class, () -> new AltSvcHandler(0, 3600));
        }

        @Test
        @DisplayName("constructor rejects negative maxAge")
        void constructorRejectsNegativeMaxAge() {
            assertThrows(IllegalArgumentException.class, () -> new AltSvcHandler(443, -1));
        }

        @Test
        @DisplayName("accepts maxAge=0 (clears cached Alt-Svc per RFC 7838)")
        void acceptsZeroMaxAge() {
            AltSvcHandler handler = new AltSvcHandler(443, 0);
            EmbeddedChannel channel = new EmbeddedChannel(handler);

            DefaultFullHttpResponse response = new DefaultFullHttpResponse(
                    HttpVersion.HTTP_1_1, HttpResponseStatus.OK);
            response.headers().set(HttpHeaderNames.CONTENT_LENGTH, 0);

            channel.writeOutbound(response);

            HttpResponse outbound = channel.readOutbound();
            assertEquals("h3=\":443\"; ma=0", outbound.headers().get("alt-svc"));

            channel.close();
        }

        @Test
        @DisplayName("handler is sharable (stateless)")
        void handlerIsShareable() {
            AltSvcHandler handler = new AltSvcHandler(443, 3600);
            assertTrue(handler.isSharable());

            EmbeddedChannel ch1 = new EmbeddedChannel(handler);
            EmbeddedChannel ch2 = new EmbeddedChannel(handler);

            DefaultFullHttpResponse resp1 = new DefaultFullHttpResponse(
                    HttpVersion.HTTP_1_1, HttpResponseStatus.OK);
            resp1.headers().set(HttpHeaderNames.CONTENT_LENGTH, 0);
            ch1.writeOutbound(resp1);

            DefaultFullHttpResponse resp2 = new DefaultFullHttpResponse(
                    HttpVersion.HTTP_1_1, HttpResponseStatus.NO_CONTENT);
            ch2.writeOutbound(resp2);

            HttpResponse out1 = ch1.readOutbound();
            HttpResponse out2 = ch2.readOutbound();
            assertNotNull(out1.headers().get("alt-svc"));
            assertNotNull(out2.headers().get("alt-svc"));

            ch1.close();
            ch2.close();
        }
    }
}
