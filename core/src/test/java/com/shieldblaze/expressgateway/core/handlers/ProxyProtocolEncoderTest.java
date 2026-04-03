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
package com.shieldblaze.expressgateway.core.handlers;

import com.shieldblaze.expressgateway.configuration.transport.BackendProxyProtocolMode;
import io.netty.buffer.ByteBuf;
import io.netty.buffer.Unpooled;
import io.netty.channel.Channel;
import io.netty.channel.embedded.EmbeddedChannel;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.Timeout;

import java.net.Inet4Address;
import java.net.Inet6Address;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.net.SocketAddress;
import java.nio.charset.StandardCharsets;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Unit tests for {@link ProxyProtocolEncoder}.
 *
 * <p>The encoder is a one-shot {@link io.netty.channel.ChannelDuplexHandler} that intercepts
 * the <em>first</em> outbound {@code write()} on the backend channel, prepends the encoded
 * PROXY protocol header, then removes itself. This design guarantees the PP header always
 * precedes application data regardless of Netty's connect-future vs. channelActive ordering.
 * All tests use {@link EmbeddedChannel} for zero-network verification.</p>
 *
 * <h3>How to observe the PP header in tests</h3>
 * <p>Because the header is emitted on the first {@code write()}, tests must trigger at least
 * one outbound write (e.g., {@code backend.writeOutbound(someBuf)}) before polling the
 * outbound queue. After the first write, the outbound queue contains the PP header immediately
 * followed by the written message.</p>
 *
 * <p>A thin {@code EmbeddedChannel} subclass overrides {@code remoteAddress()} and
 * {@code localAddress()} to supply known addresses to the encoder without requiring a
 * real TCP connection.</p>
 */
@Timeout(30)
class ProxyProtocolEncoderTest {

    // -------------------------------------------------------------------------
    // Helpers
    // -------------------------------------------------------------------------

    /**
     * EmbeddedChannel that returns caller-supplied remote and local addresses.
     * Used as the frontend channel reference passed to {@link ProxyProtocolEncoder}.
     * Its pipeline is never exercised; we only need the address methods and attr map.
     */
    private static EmbeddedChannel frontendChannel(InetSocketAddress remote, InetSocketAddress local) {
        return new EmbeddedChannel() {
            @Override
            public SocketAddress remoteAddress() {
                return remote;
            }

            @Override
            public SocketAddress localAddress() {
                return local;
            }
        };
    }

    /**
     * Create a backend {@link EmbeddedChannel} loaded with the encoder.
     * The encoder does NOT write the PP header on construction or {@code channelActive} —
     * it waits for the first outbound {@code write()}. Callers must call
     * {@code backend.writeOutbound(someBuf)} before polling the outbound queue.
     */
    private static EmbeddedChannel backendChannel(BackendProxyProtocolMode mode, Channel frontend) {
        return new EmbeddedChannel(new ProxyProtocolEncoder(mode, frontend));
    }

    /** Single empty byte payload used to trigger the PP header emission in tests. */
    private static final ByteBuf TRIGGER = Unpooled.wrappedBuffer(new byte[]{0x00});

    /**
     * Write a single dummy byte to the backend channel to trigger PP header emission,
     * then return the PP header from the outbound queue (the first outbound message).
     * The dummy byte is discarded (the second outbound message).
     *
     * <p>Callers are responsible for releasing the returned {@link ByteBuf}.</p>
     */
    private static ByteBuf triggerAndPollHeader(EmbeddedChannel backend) {
        backend.writeOutbound(TRIGGER.retainedSlice());
        ByteBuf header = backend.readOutbound();
        assertNotNull(header, "Expected PP header as first outbound message after write trigger");
        // Drain the trigger byte (second outbound message)
        ByteBuf trigger = backend.readOutbound();
        if (trigger != null) {
            trigger.release();
        }
        return header;
    }

    /**
     * Read all readable bytes from {@code buf} as an ASCII string without
     * advancing the reader index.
     */
    private static String asString(ByteBuf buf) {
        return buf.toString(buf.readerIndex(), buf.readableBytes(), StandardCharsets.US_ASCII);
    }

    // -------------------------------------------------------------------------
    // V1 tests
    // -------------------------------------------------------------------------

    @Test
    void v1_ipv4_encodesCorrectHeader() {
        InetSocketAddress src = new InetSocketAddress("192.168.1.1", 12345);
        InetSocketAddress dst = new InetSocketAddress("10.0.0.1", 8080);
        EmbeddedChannel frontend = frontendChannel(src, dst);
        EmbeddedChannel backend = backendChannel(BackendProxyProtocolMode.V1, frontend);

        ByteBuf header = triggerAndPollHeader(backend);
        try {
            assertEquals("PROXY TCP4 192.168.1.1 10.0.0.1 12345 8080\r\n", asString(header));
        } finally {
            header.release();
            backend.finishAndReleaseAll();
            frontend.finishAndReleaseAll();
        }
    }

    @Test
    void v1_ipv6_encodesCorrectHeader() throws Exception {
        InetSocketAddress src = new InetSocketAddress("2001:db8::1", 54321);
        InetSocketAddress dst = new InetSocketAddress("2001:db8::2", 443);
        EmbeddedChannel frontend = frontendChannel(src, dst);
        EmbeddedChannel backend = backendChannel(BackendProxyProtocolMode.V1, frontend);

        // The encoder uses InetAddress.getHostAddress() which returns the JVM's
        // canonical string form — not necessarily the compressed notation.
        String expectedSrc = src.getAddress().getHostAddress(); // e.g. "2001:db8:0:0:0:0:0:1"
        String expectedDst = dst.getAddress().getHostAddress();

        ByteBuf header = triggerAndPollHeader(backend);
        try {
            assertEquals("PROXY TCP6 " + expectedSrc + " " + expectedDst + " 54321 443\r\n",
                    asString(header));
        } finally {
            header.release();
            backend.finishAndReleaseAll();
            frontend.finishAndReleaseAll();
        }
    }

    @Test
    void v1_nullAddress_encodesUnknown() {
        // EmbeddedChannel with no overrides returns null for both addresses
        EmbeddedChannel frontend = new EmbeddedChannel();
        EmbeddedChannel backend = backendChannel(BackendProxyProtocolMode.V1, frontend);

        ByteBuf header = triggerAndPollHeader(backend);
        try {
            assertEquals("PROXY UNKNOWN\r\n", asString(header));
        } finally {
            header.release();
            backend.finishAndReleaseAll();
            frontend.finishAndReleaseAll();
        }
    }

    // -------------------------------------------------------------------------
    // V2 tests
    // -------------------------------------------------------------------------

    /**
     * v2 IPv4 binary header layout (28 bytes total):
     * <pre>
     *   [0..11]  12-byte signature
     *   [12]     0x21  (version=2, command=PROXY)
     *   [13]     0x11  (AF_INET, STREAM)
     *   [14..15] 0x000C (addrLen = 12)
     *   [16..19] src IPv4 (192.168.1.1 = 0xC0,0xA8,0x01,0x01)
     *   [20..23] dst IPv4 (10.0.0.1   = 0x0A,0x00,0x00,0x01)
     *   [24..25] src port 12345 (0x30,0x39)
     *   [26..27] dst port 8080  (0x1F,0x90)
     * </pre>
     */
    @Test
    void v2_ipv4_encodesCorrectBinaryHeader() {
        InetSocketAddress src = new InetSocketAddress("192.168.1.1", 12345);
        InetSocketAddress dst = new InetSocketAddress("10.0.0.1", 8080);
        EmbeddedChannel frontend = frontendChannel(src, dst);
        EmbeddedChannel backend = backendChannel(BackendProxyProtocolMode.V2, frontend);

        ByteBuf header = triggerAndPollHeader(backend);
        try {
            // 12 (sig) + 4 (ver/cmd/fam/len) + 12 (addr) = 28 bytes
            assertEquals(28, header.readableBytes());

            // Signature
            byte[] sig = {0x0D, 0x0A, 0x0D, 0x0A, 0x00, 0x0D, 0x0A, 0x51, 0x55, 0x49, 0x54, 0x0A};
            for (int i = 0; i < sig.length; i++) {
                assertEquals(sig[i], header.getByte(i),
                        "Signature mismatch at byte " + i);
            }

            assertEquals((byte) 0x21, header.getByte(12), "ver/cmd must be 0x21");
            assertEquals((byte) 0x11, header.getByte(13), "family must be 0x11 (AF_INET/STREAM)");
            assertEquals(12, header.getUnsignedShort(14), "address length must be 12");

            // src IP 192.168.1.1
            assertEquals((byte) 192, header.getByte(16));
            assertEquals((byte) 168, header.getByte(17));
            assertEquals((byte) 1,   header.getByte(18));
            assertEquals((byte) 1,   header.getByte(19));

            // dst IP 10.0.0.1
            assertEquals((byte) 10, header.getByte(20));
            assertEquals((byte) 0,  header.getByte(21));
            assertEquals((byte) 0,  header.getByte(22));
            assertEquals((byte) 1,  header.getByte(23));

            // src port 12345 = 0x3039
            assertEquals(12345, header.getUnsignedShort(24));
            // dst port 8080 = 0x1F90
            assertEquals(8080, header.getUnsignedShort(26));
        } finally {
            header.release();
            backend.finishAndReleaseAll();
            frontend.finishAndReleaseAll();
        }
    }

    /**
     * v2 IPv6 binary header layout (52 bytes total):
     * <pre>
     *   [0..11]  12-byte signature
     *   [12]     0x21  (version=2, command=PROXY)
     *   [13]     0x21  (AF_INET6, STREAM)
     *   [14..15] 0x0024 (addrLen = 36)
     *   [16..31] src IPv6 (16 bytes)
     *   [32..47] dst IPv6 (16 bytes)
     *   [48..49] src port
     *   [50..51] dst port
     * </pre>
     */
    @Test
    void v2_ipv6_encodesCorrectBinaryHeader() {
        InetSocketAddress src = new InetSocketAddress("2001:db8::1", 54321);
        InetSocketAddress dst = new InetSocketAddress("2001:db8::2", 443);
        EmbeddedChannel frontend = frontendChannel(src, dst);
        EmbeddedChannel backend = backendChannel(BackendProxyProtocolMode.V2, frontend);

        ByteBuf header = triggerAndPollHeader(backend);
        try {
            // 12 (sig) + 4 (ver/cmd/fam/len) + 36 (addr) = 52 bytes
            assertEquals(52, header.readableBytes());

            assertEquals((byte) 0x21, header.getByte(12), "ver/cmd must be 0x21");
            assertEquals((byte) 0x21, header.getByte(13), "family must be 0x21 (AF_INET6/STREAM)");
            assertEquals(36, header.getUnsignedShort(14), "address length must be 36");

            // Source port at offset 48, destination port at offset 50
            assertEquals(54321, header.getUnsignedShort(48));
            assertEquals(443,   header.getUnsignedShort(50));
        } finally {
            header.release();
            backend.finishAndReleaseAll();
            frontend.finishAndReleaseAll();
        }
    }

    @Test
    void v2_nullAddress_encodesLocalCommand() {
        // Null frontend addresses → LOCAL command, no address data
        EmbeddedChannel frontend = new EmbeddedChannel();
        EmbeddedChannel backend = backendChannel(BackendProxyProtocolMode.V2, frontend);

        ByteBuf header = triggerAndPollHeader(backend);
        try {
            // 12 (sig) + 4 (ver/cmd/fam/len) + 0 (no addr) = 16 bytes
            assertEquals(16, header.readableBytes());

            // Signature bytes
            byte[] sig = {0x0D, 0x0A, 0x0D, 0x0A, 0x00, 0x0D, 0x0A, 0x51, 0x55, 0x49, 0x54, 0x0A};
            for (int i = 0; i < sig.length; i++) {
                assertEquals(sig[i], header.getByte(i),
                        "Signature mismatch at byte " + i);
            }

            assertEquals((byte) 0x20, header.getByte(12), "ver/cmd must be 0x20 (LOCAL)");
            assertEquals((byte) 0x00, header.getByte(13), "family must be 0x00 (AF_UNSPEC)");
            assertEquals(0, header.getUnsignedShort(14), "address length must be 0");
        } finally {
            header.release();
            backend.finishAndReleaseAll();
            frontend.finishAndReleaseAll();
        }
    }

    // -------------------------------------------------------------------------
    // Pipeline self-removal
    // -------------------------------------------------------------------------

    @Test
    void encoderRemovesItselfAfterFirstWrite() {
        InetSocketAddress src = new InetSocketAddress("192.168.1.1", 9999);
        InetSocketAddress dst = new InetSocketAddress("10.0.0.1", 80);
        EmbeddedChannel frontend = frontendChannel(src, dst);
        ProxyProtocolEncoder encoder = new ProxyProtocolEncoder(BackendProxyProtocolMode.V1, frontend);
        EmbeddedChannel backend = new EmbeddedChannel(encoder);

        // The encoder is still in the pipeline after channelActive (it does not
        // remove itself on channelActive, only on the first write).
        assertNotNull(backend.pipeline().get(ProxyProtocolEncoder.class),
                "ProxyProtocolEncoder must still be in pipeline before any write");

        // Trigger the first write — the encoder prepends the PP header and removes itself
        backend.writeOutbound(Unpooled.wrappedBuffer(new byte[]{1}));

        // After the first write the encoder must have removed itself
        assertNull(backend.pipeline().get(ProxyProtocolEncoder.class),
                "ProxyProtocolEncoder must remove itself after the first outbound write");

        // Drain the outbound queue (header + the byte we wrote)
        backend.finishAndReleaseAll();
        frontend.finishAndReleaseAll();
    }

    // -------------------------------------------------------------------------
    // Ordering guarantee
    // -------------------------------------------------------------------------

    @Test
    void encoderWritesHeaderBeforeApplicationData() {
        InetSocketAddress src = new InetSocketAddress("192.168.1.1", 1111);
        InetSocketAddress dst = new InetSocketAddress("10.0.0.1", 80);
        EmbeddedChannel frontend = frontendChannel(src, dst);

        EmbeddedChannel backend = backendChannel(BackendProxyProtocolMode.V1, frontend);

        // Write application data — the encoder intercepts the first write, prepends
        // the PP header, and then passes through the app data.
        ByteBuf appData = Unpooled.copiedBuffer("GET / HTTP/1.1\r\n\r\n", StandardCharsets.US_ASCII);
        backend.writeOutbound(appData);

        // First outbound message must be the PP header
        ByteBuf first = backend.readOutbound();
        assertNotNull(first, "First outbound message must be the PP header");
        try {
            assertTrue(asString(first).startsWith("PROXY TCP4 192.168.1.1 10.0.0.1"),
                    "First outbound message must be the PROXY protocol header");
        } finally {
            first.release();
        }

        // Second outbound message is the application data
        ByteBuf second = backend.readOutbound();
        assertNotNull(second, "Second outbound message must be the application data");
        try {
            assertEquals("GET / HTTP/1.1\r\n\r\n", asString(second),
                    "Second outbound message must be the application data");
        } finally {
            second.release();
        }

        // No further messages
        assertNull(backend.readOutbound(), "No additional outbound messages expected");

        backend.finishAndReleaseAll();
        frontend.finishAndReleaseAll();
    }

    /**
     * Verifies that subsequent writes after the first do NOT re-prepend the PP header.
     * The encoder must be a true one-shot — it removes itself on the first write.
     */
    @Test
    void encoderOnlyWritesHeaderOnce() {
        InetSocketAddress src = new InetSocketAddress("192.168.1.1", 2222);
        InetSocketAddress dst = new InetSocketAddress("10.0.0.1", 80);
        EmbeddedChannel frontend = frontendChannel(src, dst);
        EmbeddedChannel backend = backendChannel(BackendProxyProtocolMode.V1, frontend);

        // First write: triggers PP header prepend
        backend.writeOutbound(Unpooled.copiedBuffer("first", StandardCharsets.US_ASCII));

        // Second write: encoder is already removed — only the app data should appear
        backend.writeOutbound(Unpooled.copiedBuffer("second", StandardCharsets.US_ASCII));

        // Read all outbound messages
        ByteBuf first  = backend.readOutbound(); // PP header
        ByteBuf second = backend.readOutbound(); // "first" app data
        ByteBuf third  = backend.readOutbound(); // "second" app data
        ByteBuf fourth = backend.readOutbound(); // must be null

        assertNotNull(first,  "first outbound must be PP header");
        assertNotNull(second, "second outbound must be 'first' app data");
        assertNotNull(third,  "third outbound must be 'second' app data");
        assertNull(fourth,    "no fourth message expected");

        // Verify the PP header is NOT present in 'third' (second app data)
        String thirdText = asString(third);
        assertFalse(thirdText.startsWith("PROXY"),
                "Second app data must not be prefixed with PP header, got: " + thirdText);
        assertEquals("second", thirdText);

        first.release();
        second.release();
        third.release();
        backend.finishAndReleaseAll();
        frontend.finishAndReleaseAll();
    }

    // -------------------------------------------------------------------------
    // IPv4-mapped IPv6 normalisation
    // -------------------------------------------------------------------------

    @Test
    void v1_ipv4MappedIpv6_encodedAsIpv4() throws Exception {
        // Build an Inet6Address whose 16-byte representation is the IPv4-mapped form
        // ::ffff:192.168.1.1  (first 10 bytes=0, bytes 10-11=0xFF, bytes 12-15=IPv4).
        // InetAddress.getByAddress(byte[16]) transparently de-maps to Inet4Address on JDK 21;
        // Inet6Address.getByAddress(null, bytes, -1) bypasses that and returns a true Inet6Address.
        byte[] mapped = new byte[16];
        mapped[10] = (byte) 0xFF;
        mapped[11] = (byte) 0xFF;
        mapped[12] = (byte) 192;
        mapped[13] = (byte) 168;
        mapped[14] = (byte) 1;
        mapped[15] = (byte) 1;
        InetAddress mappedAddr = Inet6Address.getByAddress(null, mapped, -1);

        // Confirm the construction yielded a true Inet6Address (the mapped form)
        assertFalse(mappedAddr instanceof Inet4Address,
                "Inet6Address.getByAddress(null, mapped, -1) must return Inet6Address");

        InetSocketAddress src = new InetSocketAddress(mappedAddr, 9876);
        InetSocketAddress dst = new InetSocketAddress("10.0.0.2", 80);
        EmbeddedChannel frontend = frontendChannel(src, dst);
        EmbeddedChannel backend = backendChannel(BackendProxyProtocolMode.V1, frontend);

        ByteBuf header = triggerAndPollHeader(backend);
        try {
            String text = asString(header);
            assertTrue(text.startsWith("PROXY TCP4 "),
                    "IPv4-mapped address must be encoded as TCP4, got: " + text);
            assertTrue(text.contains("192.168.1.1"),
                    "Mapped address must be de-mapped to IPv4, got: " + text);
            assertFalse(text.contains("ffff"),
                    "Encoded header must not contain IPv6 notation, got: " + text);
        } finally {
            header.release();
            backend.finishAndReleaseAll();
            frontend.finishAndReleaseAll();
        }
    }

    // -------------------------------------------------------------------------
    // REAL_CLIENT_ADDRESS attribute priority
    // -------------------------------------------------------------------------

    @Test
    void realClientAddressAttributeTakesPriority() {
        // The real client IP differs from the TCP peer address. The encoder must
        // use the attribute value (the de-proxied address) over remoteAddress().
        InetSocketAddress tcpPeer       = new InetSocketAddress("10.0.0.99", 5000); // L4 peer
        InetSocketAddress realClient    = new InetSocketAddress("203.0.113.42", 7777); // actual client
        InetSocketAddress localEndpoint = new InetSocketAddress("10.0.0.1", 443);

        EmbeddedChannel frontend = frontendChannel(tcpPeer, localEndpoint);
        // Simulate an inbound PP header having been decoded upstream
        frontend.attr(ProxyProtocolHandler.REAL_CLIENT_ADDRESS).set(realClient);

        EmbeddedChannel backend = backendChannel(BackendProxyProtocolMode.V1, frontend);

        ByteBuf header = triggerAndPollHeader(backend);
        try {
            String text = asString(header);
            assertTrue(text.contains("203.0.113.42"),
                    "Header must contain the REAL_CLIENT_ADDRESS IP, got: " + text);
            assertFalse(text.contains("10.0.0.99"),
                    "Header must not contain the raw TCP peer address, got: " + text);
            assertTrue(text.contains("7777"),
                    "Header must contain the REAL_CLIENT_ADDRESS port, got: " + text);
        } finally {
            header.release();
            backend.finishAndReleaseAll();
            frontend.finishAndReleaseAll();
        }
    }

    // -------------------------------------------------------------------------
    // Constructor guard
    // -------------------------------------------------------------------------

    @Test
    void offMode_throwsOnConstruction() {
        EmbeddedChannel frontend = new EmbeddedChannel();
        assertThrows(IllegalArgumentException.class,
                () -> new ProxyProtocolEncoder(BackendProxyProtocolMode.OFF, frontend),
                "OFF mode must be rejected at construction time");
        frontend.finishAndReleaseAll();
    }
}
