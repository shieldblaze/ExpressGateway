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
package com.shieldblaze.expressgateway.protocol.quic;

import io.netty.buffer.ByteBuf;
import io.netty.buffer.Unpooled;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.Timeout;

import java.net.InetAddress;
import java.net.InetSocketAddress;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * RB-TEST-02: Unit tests for {@link SecureQuicTokenHandler}.
 *
 * <p>Validates the HMAC-based anti-amplification token mechanism per RFC 9000 Section 8.1.
 * Tests cover:
 * <ul>
 *   <li>writeToken + validateToken round-trip for valid tokens</li>
 *   <li>Rejection of tokens from different IP addresses (address binding)</li>
 *   <li>Rejection of tampered tokens</li>
 *   <li>Rejection of truncated/empty tokens</li>
 *   <li>maxTokenLength consistency</li>
 *   <li>Different handler instances (different keys) reject each other's tokens</li>
 * </ul>
 */
@Timeout(value = 30)
class SecureQuicTokenHandlerTest {

    @Test
    void writeAndValidate_roundTrip_succeeds() throws Exception {
        byte[] key = new byte[32];
        for (int i = 0; i < 32; i++) key[i] = (byte) i;
        SecureQuicTokenHandler handler = new SecureQuicTokenHandler(key);

        InetSocketAddress address = new InetSocketAddress(InetAddress.getByName("192.168.1.100"), 12345);
        ByteBuf dcid = Unpooled.wrappedBuffer(new byte[]{1, 2, 3, 4, 5, 6, 7, 8});

        // Write token
        ByteBuf tokenBuf = Unpooled.buffer(handler.maxTokenLength());
        assertTrue(handler.writeToken(tokenBuf, dcid, address), "writeToken must succeed");
        assertEquals(32, tokenBuf.readableBytes(), "Token must be 32 bytes (HMAC-SHA256 output)");

        // Validate same address -- must succeed
        int consumed = handler.validateToken(tokenBuf, address);
        assertEquals(32, consumed, "validateToken must consume 32 bytes on success");

        dcid.release();
        tokenBuf.release();
    }

    @Test
    void validate_differentAddress_rejected() throws Exception {
        byte[] key = new byte[32];
        for (int i = 0; i < 32; i++) key[i] = (byte) i;
        SecureQuicTokenHandler handler = new SecureQuicTokenHandler(key);

        InetSocketAddress originalAddress = new InetSocketAddress(InetAddress.getByName("192.168.1.100"), 12345);
        InetSocketAddress differentAddress = new InetSocketAddress(InetAddress.getByName("10.0.0.1"), 54321);
        ByteBuf dcid = Unpooled.wrappedBuffer(new byte[]{1, 2, 3, 4});

        // Write token for original address
        ByteBuf tokenBuf = Unpooled.buffer(handler.maxTokenLength());
        assertTrue(handler.writeToken(tokenBuf, dcid, originalAddress));

        // Validate with different address -- must fail
        int result = handler.validateToken(tokenBuf, differentAddress);
        assertEquals(-1, result, "Token from a different IP must be rejected");

        dcid.release();
        tokenBuf.release();
    }

    @Test
    void validate_tamperedToken_rejected() throws Exception {
        byte[] key = new byte[32];
        for (int i = 0; i < 32; i++) key[i] = (byte) i;
        SecureQuicTokenHandler handler = new SecureQuicTokenHandler(key);

        InetSocketAddress address = new InetSocketAddress(InetAddress.getByName("192.168.1.100"), 12345);
        ByteBuf dcid = Unpooled.wrappedBuffer(new byte[]{1, 2, 3, 4});

        // Write valid token
        ByteBuf tokenBuf = Unpooled.buffer(handler.maxTokenLength());
        handler.writeToken(tokenBuf, dcid, address);

        // Tamper with one byte in the middle of the token
        int idx = tokenBuf.readerIndex() + 16;
        byte original = tokenBuf.getByte(idx);
        tokenBuf.setByte(idx, original ^ 0xFF);

        // Validate tampered token -- must fail
        int result = handler.validateToken(tokenBuf, address);
        assertEquals(-1, result, "Tampered token must be rejected");

        dcid.release();
        tokenBuf.release();
    }

    @Test
    void validate_truncatedToken_rejected() throws Exception {
        SecureQuicTokenHandler handler = new SecureQuicTokenHandler();

        InetSocketAddress address = new InetSocketAddress(InetAddress.getByName("127.0.0.1"), 443);

        // Token shorter than HMAC_LENGTH (32 bytes)
        ByteBuf shortToken = Unpooled.wrappedBuffer(new byte[16]);
        int result = handler.validateToken(shortToken, address);
        assertEquals(-1, result, "Truncated token (16 bytes) must be rejected");
        shortToken.release();

        // Empty token
        ByteBuf emptyToken = Unpooled.buffer(0);
        result = handler.validateToken(emptyToken, address);
        assertEquals(-1, result, "Empty token must be rejected");
        emptyToken.release();
    }

    @Test
    void validate_oversizedToken_rejected() throws Exception {
        SecureQuicTokenHandler handler = new SecureQuicTokenHandler();

        InetSocketAddress address = new InetSocketAddress(InetAddress.getByName("127.0.0.1"), 443);

        // Token longer than HMAC_LENGTH
        ByteBuf longToken = Unpooled.wrappedBuffer(new byte[64]);
        int result = handler.validateToken(longToken, address);
        assertEquals(-1, result, "Oversized token (64 bytes) must be rejected");
        longToken.release();
    }

    @Test
    void maxTokenLength_returns32() {
        SecureQuicTokenHandler handler = new SecureQuicTokenHandler();
        assertEquals(32, handler.maxTokenLength(), "maxTokenLength must be 32 (HMAC-SHA256 output)");
    }

    @Test
    void differentKeys_rejectEachOthersTokens() throws Exception {
        byte[] key1 = new byte[32];
        byte[] key2 = new byte[32];
        for (int i = 0; i < 32; i++) {
            key1[i] = (byte) i;
            key2[i] = (byte) (i + 100);
        }

        SecureQuicTokenHandler handler1 = new SecureQuicTokenHandler(key1);
        SecureQuicTokenHandler handler2 = new SecureQuicTokenHandler(key2);

        InetSocketAddress address = new InetSocketAddress(InetAddress.getByName("192.168.1.1"), 443);
        ByteBuf dcid = Unpooled.wrappedBuffer(new byte[]{0x0A, 0x0B, 0x0C});

        // Write token with handler1's key
        ByteBuf tokenBuf = Unpooled.buffer(handler1.maxTokenLength());
        assertTrue(handler1.writeToken(tokenBuf, dcid, address));

        // Validate with handler2's key -- must fail
        int result = handler2.validateToken(tokenBuf, address);
        assertEquals(-1, result, "Token from different key must be rejected");

        dcid.release();
        tokenBuf.release();
    }

    @Test
    void validate_ipv6Address_roundTrip() throws Exception {
        SecureQuicTokenHandler handler = new SecureQuicTokenHandler();

        InetSocketAddress ipv6Address = new InetSocketAddress(
                InetAddress.getByName("2001:db8::1"), 443);
        ByteBuf dcid = Unpooled.wrappedBuffer(new byte[]{1, 2, 3, 4, 5, 6, 7, 8});

        // Write and validate token for IPv6 address
        ByteBuf tokenBuf = Unpooled.buffer(handler.maxTokenLength());
        assertTrue(handler.writeToken(tokenBuf, dcid, ipv6Address));

        int consumed = handler.validateToken(tokenBuf, ipv6Address);
        assertEquals(32, consumed, "IPv6 token round-trip must succeed");

        // Validate with different IPv6 address -- must fail
        InetSocketAddress differentIpv6 = new InetSocketAddress(
                InetAddress.getByName("2001:db8::2"), 443);
        tokenBuf = Unpooled.buffer(handler.maxTokenLength());
        handler.writeToken(tokenBuf, dcid, ipv6Address);
        int result = handler.validateToken(tokenBuf, differentIpv6);
        assertEquals(-1, result, "Token bound to different IPv6 address must be rejected");

        dcid.release();
        tokenBuf.release();
    }

    @Test
    void sameAddress_differentPorts_produceSameToken() throws Exception {
        byte[] key = new byte[32];
        for (int i = 0; i < 32; i++) key[i] = (byte) i;
        SecureQuicTokenHandler handler = new SecureQuicTokenHandler(key);

        // Same IP, different ports -- token is based on IP only, not port.
        // This is correct per RFC 9000 Section 8.1: address validation binds to
        // the client's IP address, not the ephemeral source port.
        InetSocketAddress addr1 = new InetSocketAddress(InetAddress.getByName("192.168.1.1"), 1234);
        InetSocketAddress addr2 = new InetSocketAddress(InetAddress.getByName("192.168.1.1"), 5678);
        ByteBuf dcid = Unpooled.wrappedBuffer(new byte[]{1, 2, 3, 4});

        ByteBuf token1 = Unpooled.buffer(handler.maxTokenLength());
        handler.writeToken(token1, dcid, addr1);

        // Token written for port 1234 must validate for port 5678 (same IP)
        int consumed = handler.validateToken(token1, addr2);
        assertEquals(32, consumed, "Token must validate for same IP with different port");

        dcid.release();
        token1.release();
    }
}
