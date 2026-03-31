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
import io.netty.handler.codec.quic.QuicTokenHandler;

import javax.crypto.Mac;
import javax.crypto.spec.SecretKeySpec;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.security.InvalidKeyException;
import java.security.NoSuchAlgorithmException;
import java.security.SecureRandom;

/**
 * HMAC-based QUIC token handler for address validation per RFC 9000 Section 8.
 *
 * <p>This handler prevents amplification attacks by requiring clients to prove
 * ownership of their source address before the server sends more than 3x the
 * data it has received (RFC 9000 Section 8.1). The token is an HMAC-SHA256
 * over the client's IP address, ensuring that only the legitimate address
 * holder can produce a valid Retry token.</p>
 *
 * <h3>Token Format</h3>
 * <pre>
 * [HMAC-SHA256(key, clientIP)] — 32 bytes
 * </pre>
 *
 * <h3>Security Properties</h3>
 * <ul>
 *   <li>Key is randomly generated at startup (256-bit) — cannot be predicted by attackers</li>
 *   <li>HMAC binds the token to the client's IP address — prevents token replay from different IPs</li>
 *   <li>Tokens have no expiry (address validation is per-connection, not time-bounded)</li>
 * </ul>
 */
public final class SecureQuicTokenHandler implements QuicTokenHandler {

    private static final String HMAC_ALGO = "HmacSHA256";
    private static final int HMAC_LENGTH = 32; // SHA-256 output
    private static final SecureRandom SECURE_RANDOM = new SecureRandom();

    public static final SecureQuicTokenHandler INSTANCE = new SecureQuicTokenHandler();

    private final SecretKeySpec secretKey;

    // PERF-2: Cache Mac instance per thread to avoid Mac.getInstance() JCE provider
    // lookup on every QUIC token validation. Mac is not thread-safe, so ThreadLocal
    // is required. The Mac is initialized with the secret key on first access per thread.
    private final ThreadLocal<Mac> threadLocalMac;

    public SecureQuicTokenHandler() {
        byte[] key = new byte[32];
        SECURE_RANDOM.nextBytes(key);
        this.secretKey = new SecretKeySpec(key, HMAC_ALGO);
        this.threadLocalMac = ThreadLocal.withInitial(this::createMac);
    }

    SecureQuicTokenHandler(byte[] key) {
        this.secretKey = new SecretKeySpec(key, HMAC_ALGO);
        this.threadLocalMac = ThreadLocal.withInitial(this::createMac);
    }

    private Mac createMac() {
        try {
            Mac mac = Mac.getInstance(HMAC_ALGO);
            mac.init(secretKey);
            return mac;
        } catch (NoSuchAlgorithmException | InvalidKeyException e) {
            throw new IllegalStateException("Failed to initialize HMAC", e);
        }
    }

    @Override
    public boolean writeToken(ByteBuf out, ByteBuf dcid, InetSocketAddress address) {
        byte[] hmac = computeHmac(address.getAddress());
        out.writeBytes(hmac);
        return true;
    }

    @Override
    public int validateToken(ByteBuf token, InetSocketAddress address) {
        if (token.readableBytes() != HMAC_LENGTH) {
            return -1; // Invalid token length
        }

        byte[] expected = computeHmac(address.getAddress());

        // Constant-time comparison to prevent timing attacks
        byte[] actual = new byte[HMAC_LENGTH];
        token.readBytes(actual);
        if (constantTimeEquals(expected, actual)) {
            return HMAC_LENGTH; // Token consumed
        }
        return -1; // Validation failed
    }

    @Override
    public int maxTokenLength() {
        return HMAC_LENGTH;
    }

    private byte[] computeHmac(InetAddress address) {
        // PERF-2: Reuse ThreadLocal Mac instance instead of Mac.getInstance() per call.
        // Mac.doFinal() resets the Mac to its initialized state, so no explicit reset needed.
        Mac mac = threadLocalMac.get();
        return mac.doFinal(address.getAddress());
    }

    private static boolean constantTimeEquals(byte[] a, byte[] b) {
        if (a.length != b.length) {
            return false;
        }
        int result = 0;
        for (int i = 0; i < a.length; i++) {
            result |= a[i] ^ b[i];
        }
        return result == 0;
    }
}
