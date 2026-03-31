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
package com.shieldblaze.expressgateway.protocol.quic.retry;

import io.netty.buffer.ByteBuf;
import io.netty.buffer.Unpooled;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.Timeout;

import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.security.SecureRandom;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Tests for {@link StatelessRetryHandler} and {@link RetryTokenCodec} -- stateless
 * QUIC Retry token generation and validation per RFC 9000 Section 8.1.
 */
@Timeout(10)
class StatelessRetryHandlerTest {

    private static final SecureRandom SECURE_RANDOM = new SecureRandom();

    @Test
    void writeAndValidate_roundTrip_succeeds() throws Exception {
        byte[] key = new byte[16];
        SECURE_RANDOM.nextBytes(key);
        RetryTokenCodec codec = new RetryTokenCodec(key);
        StatelessRetryHandler handler = new StatelessRetryHandler(codec, 30_000);

        InetSocketAddress address = new InetSocketAddress(InetAddress.getByName("192.168.1.100"), 12345);
        ByteBuf dcid = Unpooled.wrappedBuffer(new byte[]{1, 2, 3, 4, 5, 6, 7, 8});

        // Write token
        ByteBuf tokenBuf = Unpooled.buffer(handler.maxTokenLength());
        assertTrue(handler.writeToken(tokenBuf, dcid, address));
        assertTrue(tokenBuf.readableBytes() > 0, "Token must be non-empty");

        // Validate same address
        int consumed = handler.validateToken(tokenBuf, address);
        assertTrue(consumed > 0, "Token validation must succeed for same address");

        assertEquals(1, handler.tokensIssued());
        assertEquals(1, handler.tokensValidated());

        dcid.release();
        tokenBuf.release();
    }

    @Test
    void validate_differentAddress_rejected() throws Exception {
        StatelessRetryHandler handler = new StatelessRetryHandler();

        InetSocketAddress original = new InetSocketAddress(InetAddress.getByName("192.168.1.100"), 12345);
        InetSocketAddress different = new InetSocketAddress(InetAddress.getByName("10.0.0.1"), 54321);
        ByteBuf dcid = Unpooled.wrappedBuffer(new byte[]{1, 2, 3, 4});

        ByteBuf tokenBuf = Unpooled.buffer(handler.maxTokenLength());
        handler.writeToken(tokenBuf, dcid, original);

        int result = handler.validateToken(tokenBuf, different);
        assertEquals(-1, result, "Token from different IP must be rejected");
        assertEquals(1, handler.tokensRejected());

        dcid.release();
        tokenBuf.release();
    }

    @Test
    void validate_expiredToken_rejected() throws Exception {
        byte[] key = new byte[16];
        SECURE_RANDOM.nextBytes(key);
        RetryTokenCodec codec = new RetryTokenCodec(key);
        // Very short expiry: 1ms
        StatelessRetryHandler handler = new StatelessRetryHandler(codec, 1);

        InetSocketAddress address = new InetSocketAddress(InetAddress.getByName("192.168.1.100"), 12345);
        ByteBuf dcid = Unpooled.wrappedBuffer(new byte[]{1, 2, 3, 4});

        ByteBuf tokenBuf = Unpooled.buffer(handler.maxTokenLength());
        handler.writeToken(tokenBuf, dcid, address);

        // Wait for token to expire
        Thread.sleep(50);

        int result = handler.validateToken(tokenBuf, address);
        assertEquals(-1, result, "Expired token must be rejected");
        assertEquals(1, handler.tokensExpired());

        dcid.release();
        tokenBuf.release();
    }

    @Test
    void validate_tamperedToken_rejected() throws Exception {
        StatelessRetryHandler handler = new StatelessRetryHandler();

        InetSocketAddress address = new InetSocketAddress(InetAddress.getByName("192.168.1.100"), 12345);
        ByteBuf dcid = Unpooled.wrappedBuffer(new byte[]{1, 2, 3, 4});

        ByteBuf tokenBuf = Unpooled.buffer(handler.maxTokenLength());
        handler.writeToken(tokenBuf, dcid, address);

        // Tamper with ciphertext (after the 12-byte nonce)
        if (tokenBuf.readableBytes() > 14) {
            int idx = tokenBuf.readerIndex() + 14;
            tokenBuf.setByte(idx, tokenBuf.getByte(idx) ^ 0xFF);
        }

        int result = handler.validateToken(tokenBuf, address);
        assertEquals(-1, result, "Tampered token must be rejected (GCM auth failure)");

        dcid.release();
        tokenBuf.release();
    }

    @Test
    void validate_emptyToken_rejected() throws Exception {
        StatelessRetryHandler handler = new StatelessRetryHandler();
        InetSocketAddress address = new InetSocketAddress(InetAddress.getByName("127.0.0.1"), 443);

        ByteBuf empty = Unpooled.buffer(0);
        int result = handler.validateToken(empty, address);
        assertEquals(-1, result, "Empty token must be rejected");
        empty.release();
    }

    @Test
    void validate_truncatedToken_rejected() throws Exception {
        StatelessRetryHandler handler = new StatelessRetryHandler();
        InetSocketAddress address = new InetSocketAddress(InetAddress.getByName("127.0.0.1"), 443);

        ByteBuf shortToken = Unpooled.wrappedBuffer(new byte[10]);
        int result = handler.validateToken(shortToken, address);
        assertEquals(-1, result, "Truncated token must be rejected");
        shortToken.release();
    }

    @Test
    void retryTokenCodec_encode_decode_roundTrip() throws Exception {
        byte[] key = new byte[16];
        SECURE_RANDOM.nextBytes(key);
        RetryTokenCodec codec = new RetryTokenCodec(key);

        InetSocketAddress address = new InetSocketAddress(InetAddress.getByName("192.168.1.100"), 12345);
        byte[] dcid = {0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F, 0x10, 0x11};

        byte[] token = codec.encode(address, dcid);
        assertNotNull(token);
        assertTrue(token.length > 0);

        RetryTokenCodec.DecodedToken decoded = codec.decode(token, address);
        assertNotNull(decoded, "Valid token must decode successfully");
        assertEquals(address.getAddress(), decoded.clientAddress());

        byte[] recoveredDcid = decoded.originalDcid();
        assertEquals(dcid.length, recoveredDcid.length);
        for (int i = 0; i < dcid.length; i++) {
            assertEquals(dcid[i], recoveredDcid[i], "Original DCID must be recovered exactly");
        }
    }

    @Test
    void retryTokenCodec_decode_wrongKey_returnsNull() throws Exception {
        byte[] key1 = new byte[16];
        byte[] key2 = new byte[16];
        SECURE_RANDOM.nextBytes(key1);
        SECURE_RANDOM.nextBytes(key2);

        RetryTokenCodec codec1 = new RetryTokenCodec(key1);
        RetryTokenCodec codec2 = new RetryTokenCodec(key2);

        InetSocketAddress address = new InetSocketAddress(InetAddress.getByName("192.168.1.1"), 443);
        byte[] token = codec1.encode(address, new byte[]{1, 2, 3, 4});

        RetryTokenCodec.DecodedToken result = codec2.decode(token, address);
        assertNull(result, "Token encrypted with different key must fail decryption");
    }

    @Test
    void retryTokenCodec_ipv6_roundTrip() throws Exception {
        RetryTokenCodec codec = new RetryTokenCodec();
        InetSocketAddress ipv6 = new InetSocketAddress(InetAddress.getByName("2001:db8::1"), 443);
        byte[] dcid = {1, 2, 3, 4, 5, 6, 7, 8};

        byte[] token = codec.encode(ipv6, dcid);
        RetryTokenCodec.DecodedToken decoded = codec.decode(token, ipv6);
        assertNotNull(decoded, "IPv6 token round-trip must succeed");
        assertEquals(ipv6.getAddress(), decoded.clientAddress());
    }

    @Test
    void retryTokenCodec_tokenExpiry() throws Exception {
        RetryTokenCodec codec = new RetryTokenCodec();
        InetSocketAddress address = new InetSocketAddress(InetAddress.getByName("192.168.1.1"), 443);
        byte[] dcid = {1, 2, 3, 4};

        byte[] token = codec.encode(address, dcid);

        // Small delay to ensure time difference between encode and check
        Thread.sleep(5);

        RetryTokenCodec.DecodedToken decoded = codec.decode(token, address);
        assertNotNull(decoded);

        long now = System.currentTimeMillis();
        assertTrue(now - decoded.timestampMs() < 1000, "Token timestamp must be recent");
        // Token is at least 5ms old, so with a 1ms window it must be expired
        assertTrue(decoded.isExpired(now, 1), "Token older than maxAge must be expired");
        // But with a 30s window it should NOT be expired
        assertTrue(!decoded.isExpired(now, 30_000), "Token within maxAge must not be expired");
    }

    @Test
    void maxTokenLength_reasonable() {
        StatelessRetryHandler handler = new StatelessRetryHandler();
        int maxLen = handler.maxTokenLength();
        assertTrue(maxLen > 0 && maxLen <= 256, "Max token length must be bounded and reasonable");
    }

    @Test
    void differentKeys_rejectEachOthersTokens() throws Exception {
        byte[] key1 = new byte[16];
        byte[] key2 = new byte[16];
        SECURE_RANDOM.nextBytes(key1);
        SECURE_RANDOM.nextBytes(key2);

        StatelessRetryHandler handler1 = new StatelessRetryHandler(new RetryTokenCodec(key1), 30_000);
        StatelessRetryHandler handler2 = new StatelessRetryHandler(new RetryTokenCodec(key2), 30_000);

        InetSocketAddress address = new InetSocketAddress(InetAddress.getByName("192.168.1.1"), 443);
        ByteBuf dcid = Unpooled.wrappedBuffer(new byte[]{1, 2, 3, 4});

        ByteBuf tokenBuf = Unpooled.buffer(handler1.maxTokenLength());
        handler1.writeToken(tokenBuf, dcid, address);

        int result = handler2.validateToken(tokenBuf, address);
        assertEquals(-1, result, "Token from different key must be rejected");

        dcid.release();
        tokenBuf.release();
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Token anti-replay
    // ═══════════════════════════════════════════════════════════════════════

    @Test
    void tokenAntiReplay_secondUse_rejected() throws Exception {
        byte[] key = new byte[16];
        SECURE_RANDOM.nextBytes(key);
        RetryTokenCodec codec = new RetryTokenCodec(key);
        StatelessRetryHandler handler = new StatelessRetryHandler(codec, 30_000);

        InetSocketAddress address = new InetSocketAddress(InetAddress.getByName("192.168.1.100"), 12345);
        ByteBuf dcid = Unpooled.wrappedBuffer(new byte[]{1, 2, 3, 4, 5, 6, 7, 8});

        // Issue a token
        ByteBuf tokenBuf = Unpooled.buffer(handler.maxTokenLength());
        handler.writeToken(tokenBuf, dcid, address);

        // Copy the token bytes for a second validation attempt
        byte[] tokenCopy = new byte[tokenBuf.readableBytes()];
        tokenBuf.getBytes(tokenBuf.readerIndex(), tokenCopy);

        // First validation: should succeed
        int result1 = handler.validateToken(tokenBuf, address);
        assertTrue(result1 > 0, "First use of token must succeed");
        assertEquals(1, handler.tokensValidated());

        // Second validation with the same token bytes: should be rejected (replay)
        ByteBuf replayBuf = Unpooled.wrappedBuffer(tokenCopy);
        int result2 = handler.validateToken(replayBuf, address);
        assertEquals(-1, result2, "Second use of same token must be rejected (anti-replay)");
        assertEquals(1, handler.tokensReplayed());

        dcid.release();
        replayBuf.release();
    }

    @Test
    void tokenAntiReplay_differentTokens_bothAccepted() throws Exception {
        byte[] key = new byte[16];
        SECURE_RANDOM.nextBytes(key);
        RetryTokenCodec codec = new RetryTokenCodec(key);
        StatelessRetryHandler handler = new StatelessRetryHandler(codec, 30_000);

        InetSocketAddress address = new InetSocketAddress(InetAddress.getByName("192.168.1.100"), 12345);
        ByteBuf dcid = Unpooled.wrappedBuffer(new byte[]{1, 2, 3, 4});

        // Issue two different tokens (different nonces -> different ciphertext)
        ByteBuf tokenBuf1 = Unpooled.buffer(handler.maxTokenLength());
        handler.writeToken(tokenBuf1, dcid, address);

        ByteBuf tokenBuf2 = Unpooled.buffer(handler.maxTokenLength());
        dcid.resetReaderIndex();
        handler.writeToken(tokenBuf2, dcid, address);

        // Both should be accepted (different tokens)
        int result1 = handler.validateToken(tokenBuf1, address);
        assertTrue(result1 > 0, "First distinct token must be accepted");

        int result2 = handler.validateToken(tokenBuf2, address);
        assertTrue(result2 > 0, "Second distinct token must be accepted");

        assertEquals(2, handler.tokensValidated());
        assertEquals(0, handler.tokensReplayed());

        dcid.release();
        tokenBuf1.release();
        tokenBuf2.release();
    }
}
