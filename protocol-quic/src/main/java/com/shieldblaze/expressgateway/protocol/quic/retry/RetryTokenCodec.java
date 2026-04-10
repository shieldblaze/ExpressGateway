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

import javax.crypto.Cipher;
import javax.crypto.SecretKey;
import javax.crypto.spec.GCMParameterSpec;
import javax.crypto.spec.SecretKeySpec;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.nio.ByteBuffer;
import java.security.SecureRandom;

/**
 * AES-128-GCM based codec for QUIC Retry tokens per RFC 9000 Section 8.1.
 *
 * <h3>Token Format (encrypted)</h3>
 * <pre>
 * [nonce : 12 bytes][ciphertext : variable][GCM tag : 16 bytes]
 * </pre>
 *
 * <h3>Plaintext Structure (before encryption)</h3>
 * <pre>
 * [timestamp_ms : 8 bytes][client_ip_len : 1 byte][client_ip : 4 or 16 bytes][original_dcid_len : 1 byte][original_dcid : N bytes]
 * </pre>
 *
 * <h3>Security Properties</h3>
 * <ul>
 *   <li>AES-128-GCM provides authenticated encryption -- tampered tokens are detected</li>
 *   <li>Random 12-byte nonce per token prevents deterministic output</li>
 *   <li>Client IP is embedded and validated on decode -- prevents token relay from different IPs</li>
 *   <li>Timestamp enables expiry checks without maintaining server-side state</li>
 *   <li>Original DCID is preserved for connection establishment (RFC 9000 Section 7.3)</li>
 * </ul>
 *
 * <h3>Thread Safety</h3>
 * <p>Thread-safe via ThreadLocal Cipher instances. The key is immutable and shared.</p>
 */
public final class RetryTokenCodec {

    private static final String CIPHER_ALGO = "AES/GCM/NoPadding";
    private static final int GCM_TAG_BITS = 128;
    private static final int GCM_TAG_BYTES = 16;
    private static final int NONCE_LENGTH = 12;

    /**
     * Decoded retry token contents.
     *
     * @param timestampMs   when the token was issued (epoch milliseconds)
     * @param clientAddress the client IP address embedded in the token
     * @param originalDcid  the original DCID from the client's Initial packet
     */
    public record DecodedToken(long timestampMs, InetAddress clientAddress, byte[] originalDcid) {

        /**
         * Check if this token has expired.
         *
         * @param nowMs current time in epoch milliseconds
         * @param maxAgeMs maximum token age in milliseconds
         * @return true if the token is expired
         */
        public boolean isExpired(long nowMs, long maxAgeMs) {
            return nowMs - timestampMs > maxAgeMs;
        }
    }

    private static final SecureRandom SECURE_RANDOM = new SecureRandom();

    private final SecretKey secretKey;
    private final ThreadLocal<SecureRandom> threadLocalRandom = ThreadLocal.withInitial(SecureRandom::new);
    private final ThreadLocal<Cipher> threadLocalCipher = ThreadLocal.withInitial(() -> {
        try {
            return Cipher.getInstance(CIPHER_ALGO);
        } catch (Exception e) {
            throw new IllegalStateException("AES-GCM not available", e);
        }
    });

    /**
     * Create a new RetryTokenCodec with a randomly generated 128-bit key.
     */
    public RetryTokenCodec() {
        byte[] keyBytes = new byte[16];
        SECURE_RANDOM.nextBytes(keyBytes);
        this.secretKey = new SecretKeySpec(keyBytes, "AES");
    }

    /**
     * Create a RetryTokenCodec with a specified key (for testing or cluster key distribution).
     *
     * @param key the 16-byte AES key
     */
    public RetryTokenCodec(byte[] key) {
        if (key.length != 16) {
            throw new IllegalArgumentException("AES key must be 16 bytes, got: " + key.length);
        }
        this.secretKey = new SecretKeySpec(key, "AES");
    }

    /**
     * Encode a retry token.
     *
     * @param clientAddress the client's socket address (IP bound in token)
     * @param originalDcid  the original DCID from the client's Initial packet
     * @return the encrypted token bytes
     */
    public byte[] encode(InetSocketAddress clientAddress, byte[] originalDcid) {
        byte[] clientIp = clientAddress.getAddress().getAddress();
        long timestampMs = System.currentTimeMillis();

        // Plaintext: [timestamp:8][ip_len:1][ip:4/16][dcid_len:1][dcid:N]
        int plaintextLen = 8 + 1 + clientIp.length + 1 + originalDcid.length;
        ByteBuffer plaintext = ByteBuffer.allocate(plaintextLen);
        plaintext.putLong(timestampMs);
        plaintext.put((byte) clientIp.length);
        plaintext.put(clientIp);
        plaintext.put((byte) originalDcid.length);
        plaintext.put(originalDcid);
        plaintext.flip();

        // Generate random nonce
        byte[] nonce = new byte[NONCE_LENGTH];
        threadLocalRandom.get().nextBytes(nonce);

        try {
            Cipher cipher = threadLocalCipher.get();
            GCMParameterSpec gcmSpec = new GCMParameterSpec(GCM_TAG_BITS, nonce);
            cipher.init(Cipher.ENCRYPT_MODE, secretKey, gcmSpec);
            byte[] ciphertext = cipher.doFinal(plaintext.array());

            // Output: [nonce:12][ciphertext+tag]
            byte[] token = new byte[NONCE_LENGTH + ciphertext.length];
            System.arraycopy(nonce, 0, token, 0, NONCE_LENGTH);
            System.arraycopy(ciphertext, 0, token, NONCE_LENGTH, ciphertext.length);
            return token;
        } catch (Exception e) {
            throw new IllegalStateException("Failed to encrypt retry token", e);
        }
    }

    /**
     * Decode and validate a retry token.
     *
     * @param token the encrypted token bytes
     * @param expectedAddress the client address to validate against
     * @return the decoded token, or null if decryption fails, is tampered, or IP mismatches
     */
    public DecodedToken decode(byte[] token, InetSocketAddress expectedAddress) {
        if (token == null || token.length < NONCE_LENGTH + GCM_TAG_BYTES + 10) {
            return null; // Too short for nonce + tag + minimal plaintext
        }

        byte[] nonce = new byte[NONCE_LENGTH];
        System.arraycopy(token, 0, nonce, 0, NONCE_LENGTH);

        byte[] ciphertext = new byte[token.length - NONCE_LENGTH];
        System.arraycopy(token, NONCE_LENGTH, ciphertext, 0, ciphertext.length);

        byte[] plaintext;
        try {
            Cipher cipher = threadLocalCipher.get();
            GCMParameterSpec gcmSpec = new GCMParameterSpec(GCM_TAG_BITS, nonce);
            cipher.init(Cipher.DECRYPT_MODE, secretKey, gcmSpec);
            plaintext = cipher.doFinal(ciphertext);
        } catch (Exception _) {
            // Tampered, wrong key, or corrupt -- GCM authentication failed
            return null;
        }

        // Parse plaintext: [timestamp:8][ip_len:1][ip:4/16][dcid_len:1][dcid:N]
        if (plaintext.length < 11) { // 8 + 1 + min(1) + 1
            return null;
        }

        ByteBuffer buf = ByteBuffer.wrap(plaintext);
        long timestampMs = buf.getLong();
        int ipLen = buf.get() & 0xFF;

        if (ipLen != 4 && ipLen != 16) {
            return null; // Invalid IP length
        }
        if (buf.remaining() < ipLen + 1) {
            return null;
        }

        byte[] ipBytes = new byte[ipLen];
        buf.get(ipBytes);

        int dcidLen = buf.get() & 0xFF;
        if (buf.remaining() < dcidLen) {
            return null;
        }
        byte[] originalDcid = new byte[dcidLen];
        buf.get(originalDcid);

        // Validate client IP matches
        InetAddress tokenAddress;
        try {
            tokenAddress = InetAddress.getByAddress(ipBytes);
        } catch (Exception _) {
            return null;
        }

        if (!tokenAddress.equals(expectedAddress.getAddress())) {
            return null; // IP mismatch -- token was issued for a different client
        }

        return new DecodedToken(timestampMs, tokenAddress, originalDcid);
    }

    /**
     * Maximum encoded token length. Used for buffer pre-allocation.
     * nonce(12) + timestamp(8) + ipLen(1) + ip(16) + dcidLen(1) + dcid(20) + gcmTag(16) = 74
     */
    public int maxTokenLength() {
        return NONCE_LENGTH + 8 + 1 + 16 + 1 + 20 + GCM_TAG_BYTES;
    }
}
