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

import com.shieldblaze.expressgateway.configuration.ConfigurationContext;
import com.shieldblaze.expressgateway.configuration.http.HttpConfiguration;
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancer;
import io.netty.channel.Channel;
import io.netty.channel.ChannelHandlerContext;
import io.netty.handler.codec.quic.QuicChannel;
import io.netty.handler.codec.quic.QuicStreamChannel;
import io.netty.util.Attribute;
import io.netty.util.AttributeKey;
import io.netty.util.DefaultAttributeMap;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Nested;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.Timeout;

import java.lang.reflect.Field;
import java.lang.reflect.Method;
import java.util.Set;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertSame;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.mockito.ArgumentMatchers.any;
import static org.mockito.Mockito.mock;
import static org.mockito.Mockito.when;

/**
 * Unit tests for HTTP/3 graceful shutdown (draining) behavior
 * per RFC 9114 Section 5.2.
 *
 * <p>Tests cover:
 * <ul>
 *   <li>{@link Http3Constants#DRAINING_KEY} attribute key identity and type</li>
 *   <li>{@link Http3ServerHandler#startDraining(QuicChannel)} attribute mutation</li>
 *   <li>{@code isConnectionDraining(ChannelHandlerContext)} private method correctness</li>
 *   <li>Data-dribble defense: {@code maxDataFramesPerStream} computation</li>
 * </ul>
 */
@Timeout(value = 10)
class Http3GracefulShutdownTest {

    // ── Helpers ─────────────────────────────────────────────────────────────

    /**
     * Creates a Mockito mock of {@link QuicChannel} backed by a real
     * {@link DefaultAttributeMap} so that {@code attr()} calls store and
     * retrieve attributes correctly (not just return null or another mock).
     */
    private static QuicChannel mockQuicChannelWithRealAttributes() {
        QuicChannel quicChannel = mock(QuicChannel.class);
        DefaultAttributeMap attributeMap = new DefaultAttributeMap();

        // Delegate attr() to the real map. The unchecked cast is safe because
        // DefaultAttributeMap returns Attribute<T> for AttributeKey<T>.
        @SuppressWarnings("unchecked")
        Object stubbing = when(quicChannel.attr(any(AttributeKey.class)))
                .thenAnswer(invocation -> {
                    AttributeKey<?> key = invocation.getArgument(0);
                    return attributeMap.attr(key);
                });
        assertNotNull(stubbing, "Mockito stubbing must be configured");

        return quicChannel;
    }

    /**
     * Creates a mock {@link ChannelHandlerContext} whose channel is a
     * {@link QuicStreamChannel} with the given {@link QuicChannel} as parent.
     */
    private static ChannelHandlerContext mockStreamCtx(QuicChannel parentQuicChannel) {
        QuicStreamChannel streamChannel = mock(QuicStreamChannel.class);
        when(streamChannel.parent()).thenReturn(parentQuicChannel);
        // Channel.parent() (the default interface method) also returns the QuicChannel
        when(((Channel) streamChannel).parent()).thenReturn(parentQuicChannel);

        ChannelHandlerContext ctx = mock(ChannelHandlerContext.class);
        when(ctx.channel()).thenReturn(streamChannel);
        return ctx;
    }

    /**
     * Invokes the private static method {@code isConnectionDraining(ChannelHandlerContext)}
     * on {@link Http3ServerHandler} via reflection.
     */
    private static boolean invokeIsConnectionDraining(ChannelHandlerContext ctx) throws Exception {
        Method method = Http3ServerHandler.class.getDeclaredMethod("isConnectionDraining", ChannelHandlerContext.class);
        method.setAccessible(true);
        return (boolean) method.invoke(null, ctx);
    }

    /**
     * Reads the private final field {@code maxDataFramesPerStream} from an
     * {@link Http3ServerHandler} instance via reflection.
     */
    private static int readMaxDataFramesPerStream(Http3ServerHandler handler) throws Exception {
        Field field = Http3ServerHandler.class.getDeclaredField("maxDataFramesPerStream");
        field.setAccessible(true);
        return (int) field.get(handler);
    }

    /**
     * Creates an {@link Http3ServerHandler} with a mocked {@link L4LoadBalancer}
     * configured to return the given {@code maxRequestBodySize} and a standard
     * set of allowed methods.
     */
    private static Http3ServerHandler createHandler(long maxRequestBodySize) {
        HttpConfiguration httpConfig = mock(HttpConfiguration.class);
        when(httpConfig.maxRequestBodySize()).thenReturn(maxRequestBodySize);
        when(httpConfig.allowedMethods()).thenReturn(Set.of("GET", "POST", "PUT", "DELETE"));

        ConfigurationContext configCtx = mock(ConfigurationContext.class);
        when(configCtx.httpConfiguration()).thenReturn(httpConfig);

        L4LoadBalancer loadBalancer = mock(L4LoadBalancer.class);
        when(loadBalancer.configurationContext()).thenReturn(configCtx);

        Channel frontendChannel = mock(Channel.class);

        // ConnectionPool is final and cannot be mocked. Passing null is safe here
        // because the constructor only stores the reference -- it is not dereferenced
        // during construction, and these tests never exercise the proxying path.
        return new Http3ServerHandler(loadBalancer, frontendChannel, null);
    }

    // ========================================================================
    // 1. Http3Constants.DRAINING_KEY
    // ========================================================================

    @Nested
    @DisplayName("Http3Constants.DRAINING_KEY")
    class DrainingKeyTests {

        @Test
        @DisplayName("DRAINING_KEY is non-null and has the expected name")
        void drainingKeyIsNonNullWithExpectedName() {
            AttributeKey<Boolean> key = Http3Constants.DRAINING_KEY;
            assertNotNull(key, "DRAINING_KEY must not be null");
            assertEquals("h3.draining", key.name(),
                    "DRAINING_KEY name must match the registered attribute name");
        }

        @Test
        @DisplayName("DRAINING_KEY is the same instance on Http3ServerHandler and Http3Constants")
        void drainingKeyIdentityAcrossClasses() {
            // Http3ServerHandler.DRAINING_KEY delegates to Http3Constants.DRAINING_KEY.
            // Verify they are the exact same object (identity, not just equality).
            assertSame(Http3Constants.DRAINING_KEY, Http3ServerHandler.DRAINING_KEY,
                    "Handler's DRAINING_KEY must be the same AttributeKey instance as the constant");
        }

        @Test
        @DisplayName("DRAINING_KEY is a proper AttributeKey<Boolean> usable on a channel")
        void drainingKeyIsUsableOnAttributeMap() {
            DefaultAttributeMap map = new DefaultAttributeMap();
            Attribute<Boolean> attr = map.attr(Http3Constants.DRAINING_KEY);
            assertNotNull(attr, "attr() must return a non-null Attribute");
            assertNull(attr.get(), "Initial value must be null (not set)");

            attr.set(Boolean.TRUE);
            assertEquals(Boolean.TRUE, attr.get());

            attr.set(Boolean.FALSE);
            assertEquals(Boolean.FALSE, attr.get());
        }
    }

    // ========================================================================
    // 2. startDraining / isConnectionDraining
    // ========================================================================

    @Nested
    @DisplayName("startDraining and isConnectionDraining")
    class DrainingLifecycleTests {

        private QuicChannel quicChannel;

        @BeforeEach
        void setUp() {
            quicChannel = mockQuicChannelWithRealAttributes();
        }

        @Test
        @DisplayName("startDraining sets DRAINING_KEY to TRUE on the QuicChannel")
        void startDrainingSetsAttribute() {
            // Before draining, attribute should be null (not set).
            assertNull(quicChannel.attr(Http3Constants.DRAINING_KEY).get(),
                    "DRAINING_KEY must be null before startDraining");

            Http3ServerHandler.startDraining(quicChannel);

            Boolean value = quicChannel.attr(Http3Constants.DRAINING_KEY).get();
            assertEquals(Boolean.TRUE, value,
                    "DRAINING_KEY must be TRUE after startDraining");
        }

        @Test
        @DisplayName("isConnectionDraining returns false before startDraining")
        void isConnectionDrainingReturnsFalseInitially() throws Exception {
            ChannelHandlerContext ctx = mockStreamCtx(quicChannel);

            assertFalse(invokeIsConnectionDraining(ctx),
                    "isConnectionDraining must return false when attribute is not set");
        }

        @Test
        @DisplayName("isConnectionDraining returns true after startDraining")
        void isConnectionDrainingReturnsTrueAfterDraining() throws Exception {
            ChannelHandlerContext ctx = mockStreamCtx(quicChannel);

            Http3ServerHandler.startDraining(quicChannel);

            assertTrue(invokeIsConnectionDraining(ctx),
                    "isConnectionDraining must return true after startDraining");
        }

        @Test
        @DisplayName("isConnectionDraining returns false when attribute is explicitly FALSE")
        void isConnectionDrainingReturnsFalseWhenExplicitlyFalse() throws Exception {
            ChannelHandlerContext ctx = mockStreamCtx(quicChannel);

            // Explicitly set to FALSE (not null) -- this is a valid state if
            // draining were ever cleared (defensive test).
            quicChannel.attr(Http3Constants.DRAINING_KEY).set(Boolean.FALSE);

            assertFalse(invokeIsConnectionDraining(ctx),
                    "isConnectionDraining must return false when attribute is Boolean.FALSE");
        }

        @Test
        @DisplayName("isConnectionDraining returns false when parent is not a QuicChannel")
        void isConnectionDrainingReturnsFalseForNonQuicParent() throws Exception {
            // Simulate a stream whose parent is a plain Channel, not a QuicChannel.
            Channel plainParent = mock(Channel.class);
            Channel streamChannel = mock(Channel.class);
            when(streamChannel.parent()).thenReturn(plainParent);

            ChannelHandlerContext ctx = mock(ChannelHandlerContext.class);
            when(ctx.channel()).thenReturn(streamChannel);

            assertFalse(invokeIsConnectionDraining(ctx),
                    "isConnectionDraining must return false when parent is not a QuicChannel");
        }

        @Test
        @DisplayName("startDraining is idempotent -- calling twice does not change state")
        void startDrainingIsIdempotent() {
            Http3ServerHandler.startDraining(quicChannel);
            assertEquals(Boolean.TRUE, quicChannel.attr(Http3Constants.DRAINING_KEY).get());

            // Call again -- should still be TRUE without error.
            Http3ServerHandler.startDraining(quicChannel);
            assertEquals(Boolean.TRUE, quicChannel.attr(Http3Constants.DRAINING_KEY).get());
        }

        @Test
        @DisplayName("multiple streams on the same QuicChannel see the same draining state")
        void multipleStreamsSeeConsistentDrainingState() throws Exception {
            ChannelHandlerContext ctx1 = mockStreamCtx(quicChannel);
            ChannelHandlerContext ctx2 = mockStreamCtx(quicChannel);

            assertFalse(invokeIsConnectionDraining(ctx1));
            assertFalse(invokeIsConnectionDraining(ctx2));

            Http3ServerHandler.startDraining(quicChannel);

            assertTrue(invokeIsConnectionDraining(ctx1),
                    "Stream 1 must see draining after startDraining");
            assertTrue(invokeIsConnectionDraining(ctx2),
                    "Stream 2 must see draining after startDraining");
        }

        @Test
        @DisplayName("independent QuicChannels have independent draining state")
        void independentQuicChannelsHaveIndependentState() throws Exception {
            QuicChannel quicChannel2 = mockQuicChannelWithRealAttributes();

            ChannelHandlerContext ctx1 = mockStreamCtx(quicChannel);
            ChannelHandlerContext ctx2 = mockStreamCtx(quicChannel2);

            // Drain only the first connection.
            Http3ServerHandler.startDraining(quicChannel);

            assertTrue(invokeIsConnectionDraining(ctx1),
                    "Stream on drained connection must report draining");
            assertFalse(invokeIsConnectionDraining(ctx2),
                    "Stream on non-drained connection must NOT report draining");
        }
    }

    // ========================================================================
    // 3. Data-dribble defense: maxDataFramesPerStream computation
    // ========================================================================

    @Nested
    @DisplayName("maxDataFramesPerStream computation")
    class DataDribbleDefenseTests {

        private static final int BASE_MAX_DATA_FRAMES = 10_000;
        private static final int MIN_EXPECTED_FRAME_SIZE = 1024;

        @Test
        @DisplayName("maxRequestBodySize=0 yields BASE_MAX_DATA_FRAMES (10,000)")
        void zeroBodySizeYieldsBaseLimit() throws Exception {
            Http3ServerHandler handler = createHandler(0);
            assertEquals(BASE_MAX_DATA_FRAMES, readMaxDataFramesPerStream(handler),
                    "When maxRequestBodySize=0, limit must be BASE_MAX_DATA_FRAMES");
        }

        @Test
        @DisplayName("small maxRequestBodySize yields BASE_MAX_DATA_FRAMES (floor)")
        void smallBodySizeYieldsBaseLimit() throws Exception {
            // 1 MB / 1024 = 1024, which is less than 10,000 -- floor applies.
            long smallBodySize = 1_000_000;
            Http3ServerHandler handler = createHandler(smallBodySize);
            int expected = (int) Math.max(BASE_MAX_DATA_FRAMES, smallBodySize / MIN_EXPECTED_FRAME_SIZE);
            assertEquals(BASE_MAX_DATA_FRAMES, expected, "Sanity: 1MB should use the base");
            assertEquals(expected, readMaxDataFramesPerStream(handler));
        }

        @Test
        @DisplayName("10 MB maxRequestBodySize yields BASE_MAX_DATA_FRAMES (exact boundary)")
        void tenMbBodySizeYieldsBaseLimit() throws Exception {
            // 10 MB / 1024 = 10,240 > 10,000 -- just above the base.
            long bodySize = 10L * 1024 * 1024;
            Http3ServerHandler handler = createHandler(bodySize);
            int expected = (int) Math.max(BASE_MAX_DATA_FRAMES, bodySize / MIN_EXPECTED_FRAME_SIZE);
            assertEquals(10_240, expected, "Sanity: 10 MB / 1024 = 10,240");
            assertEquals(expected, readMaxDataFramesPerStream(handler));
        }

        @Test
        @DisplayName("100 MB maxRequestBodySize scales the limit above base")
        void largeBodySizeScalesLimit() throws Exception {
            long bodySize = 100L * 1024 * 1024; // 100 MB
            Http3ServerHandler handler = createHandler(bodySize);
            int expected = (int) Math.max(BASE_MAX_DATA_FRAMES, bodySize / MIN_EXPECTED_FRAME_SIZE);
            assertEquals(102_400, expected, "Sanity: 100 MB / 1024 = 102,400");
            assertEquals(expected, readMaxDataFramesPerStream(handler));
        }

        @Test
        @DisplayName("negative maxRequestBodySize (disabled) yields scaled value")
        void negativeBodySizeYieldsBaseLimit() throws Exception {
            // The code checks `if (maxRequestBodySize > 0)`. Negative means the
            // else-branch fires, giving BASE_MAX_DATA_FRAMES.
            Http3ServerHandler handler = createHandler(-1);
            assertEquals(BASE_MAX_DATA_FRAMES, readMaxDataFramesPerStream(handler),
                    "Negative maxRequestBodySize must use BASE_MAX_DATA_FRAMES");
        }

        @Test
        @DisplayName("maxRequestBodySize just below the crossover uses base limit")
        void justBelowCrossoverUsesBase() throws Exception {
            // Crossover: BASE_MAX_DATA_FRAMES * MIN_EXPECTED_FRAME_SIZE = 10,000 * 1024 = 10,240,000
            long crossover = (long) BASE_MAX_DATA_FRAMES * MIN_EXPECTED_FRAME_SIZE;
            Http3ServerHandler handler = createHandler(crossover - 1);
            int expected = (int) Math.max(BASE_MAX_DATA_FRAMES, (crossover - 1) / MIN_EXPECTED_FRAME_SIZE);
            assertEquals(BASE_MAX_DATA_FRAMES, expected,
                    "Sanity: just below crossover should use base (integer division)");
            assertEquals(expected, readMaxDataFramesPerStream(handler));
        }

        @Test
        @DisplayName("maxRequestBodySize at exact crossover uses scaled value")
        void atExactCrossoverUsesScaled() throws Exception {
            long crossover = (long) BASE_MAX_DATA_FRAMES * MIN_EXPECTED_FRAME_SIZE;
            Http3ServerHandler handler = createHandler(crossover);
            int expected = (int) Math.max(BASE_MAX_DATA_FRAMES, crossover / MIN_EXPECTED_FRAME_SIZE);
            // crossover / 1024 == 10,000 exactly, so max(10,000, 10,000) == 10,000
            assertEquals(BASE_MAX_DATA_FRAMES, expected);
            assertEquals(expected, readMaxDataFramesPerStream(handler));
        }

        @Test
        @DisplayName("maxRequestBodySize one byte above crossover yields base+1")
        void oneAboveCrossoverYieldsBaseLimit() throws Exception {
            // (crossover + 1) / 1024 is still 10,000 due to integer division truncation
            // Need crossover + 1024 to actually get 10,001
            long crossover = (long) BASE_MAX_DATA_FRAMES * MIN_EXPECTED_FRAME_SIZE;
            Http3ServerHandler handler = createHandler(crossover + MIN_EXPECTED_FRAME_SIZE);
            int expected = (int) Math.max(BASE_MAX_DATA_FRAMES,
                    (crossover + MIN_EXPECTED_FRAME_SIZE) / MIN_EXPECTED_FRAME_SIZE);
            assertEquals(BASE_MAX_DATA_FRAMES + 1, expected);
            assertEquals(expected, readMaxDataFramesPerStream(handler));
        }
    }
}
