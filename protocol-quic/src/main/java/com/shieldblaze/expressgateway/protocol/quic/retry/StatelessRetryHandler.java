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

import com.shieldblaze.expressgateway.protocol.quic.ByteArrayKey;
import io.netty.buffer.ByteBuf;
import io.netty.handler.codec.quic.QuicTokenHandler;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.net.InetSocketAddress;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.atomic.AtomicLong;

/**
 * Stateless QUIC Retry token handler with AES-GCM encrypted tokens.
 *
 * <h3>RFC 9000 Section 8.1: Address Validation during Connection Establishment</h3>
 * <p>The server sends a Retry packet containing a token to the client. The client
 * echoes the token in its next Initial packet. The server validates the token to
 * confirm the client owns its source address, preventing amplification attacks.</p>
 *
 * <h3>Stateless Design</h3>
 * <p>Unlike the base {@code SecureQuicTokenHandler} which uses HMAC (providing only
 * IP binding), this handler uses AES-GCM to encrypt the original DCID, timestamp,
 * and client IP into the token. This enables:
 * <ul>
 *   <li>Token expiration (time-bounded validity)</li>
 *   <li>Original DCID recovery for connection establishment (RFC 9000 Section 7.3)</li>
 *   <li>Tamper detection via GCM authentication tag</li>
 *   <li>Anti-replay via timestamp freshness check</li>
 * </ul>
 * No server-side state is required -- all information is in the encrypted token.</p>
 *
 * <h3>Integration</h3>
 * <p>Implements {@link QuicTokenHandler} for direct use with Netty's
 * {@link io.netty.handler.codec.quic.QuicServerCodecBuilder#tokenHandler}.</p>
 *
 * <h3>Thread Safety</h3>
 * <p>Stateless -- uses ThreadLocal cipher instances via {@link RetryTokenCodec}.</p>
 */
public final class StatelessRetryHandler implements QuicTokenHandler {

    private static final Logger logger = LogManager.getLogger(StatelessRetryHandler.class);

    /**
     * Default token validity window: 30 seconds.
     * Long enough for a client to receive the Retry and resend the Initial,
     * short enough to prevent token replay from cached/intercepted Retry packets.
     */
    private static final long DEFAULT_TOKEN_MAX_AGE_MS = 30_000;

    /**
     * Maximum number of used tokens to track. Bounded to prevent memory exhaustion.
     * When the limit is reached, older entries are evicted.
     */
    private static final int MAX_USED_TOKENS = 100_000;

    private final RetryTokenCodec codec;
    private final long tokenMaxAgeMs;

    /**
     * Tracks used tokens for anti-replay. A token that has been successfully validated
     * once MUST NOT be accepted again. The key is the raw token bytes, the value is
     * the timestamp when it was used (for eviction).
     */
    private final ConcurrentHashMap<ByteArrayKey, Long> usedTokens = new ConcurrentHashMap<>();

    private final AtomicLong tokensIssued = new AtomicLong();
    private final AtomicLong tokensValidated = new AtomicLong();
    private final AtomicLong tokensRejected = new AtomicLong();
    private final AtomicLong tokensExpired = new AtomicLong();
    private final AtomicLong tokensReplayed = new AtomicLong();

    /**
     * Create a StatelessRetryHandler with default settings (random key, 30s expiry).
     */
    public StatelessRetryHandler() {
        this(new RetryTokenCodec(), DEFAULT_TOKEN_MAX_AGE_MS);
    }

    /**
     * Create a StatelessRetryHandler with a specific codec and expiry.
     *
     * @param codec         the token codec to use
     * @param tokenMaxAgeMs maximum token age in milliseconds
     */
    public StatelessRetryHandler(RetryTokenCodec codec, long tokenMaxAgeMs) {
        if (tokenMaxAgeMs <= 0) {
            throw new IllegalArgumentException("tokenMaxAgeMs must be positive, got: " + tokenMaxAgeMs);
        }
        this.codec = codec;
        this.tokenMaxAgeMs = tokenMaxAgeMs;
    }

    @Override
    public boolean writeToken(ByteBuf out, ByteBuf dcid, InetSocketAddress address) {
        // Extract the DCID bytes from the ByteBuf
        byte[] dcidBytes = new byte[dcid.readableBytes()];
        dcid.getBytes(dcid.readerIndex(), dcidBytes);

        byte[] token = codec.encode(address, dcidBytes);
        out.writeBytes(token);

        tokensIssued.incrementAndGet();
        if (logger.isDebugEnabled()) {
            logger.debug("Retry token issued for address {} (token size: {} bytes)",
                    address, token.length);
        }
        return true;
    }

    @Override
    public int validateToken(ByteBuf token, InetSocketAddress address) {
        int tokenLen = token.readableBytes();
        if (tokenLen == 0) {
            tokensRejected.incrementAndGet();
            return -1;
        }

        byte[] tokenBytes = new byte[tokenLen];
        token.readBytes(tokenBytes);

        RetryTokenCodec.DecodedToken decoded = codec.decode(tokenBytes, address);
        if (decoded == null) {
            tokensRejected.incrementAndGet();
            if (logger.isDebugEnabled()) {
                logger.debug("Retry token validation failed (decrypt/IP mismatch) for address {}", address);
            }
            return -1;
        }

        // Check token expiry
        if (decoded.isExpired(System.currentTimeMillis(), tokenMaxAgeMs)) {
            tokensExpired.incrementAndGet();
            if (logger.isDebugEnabled()) {
                logger.debug("Retry token expired for address {} (age: {}ms)",
                        address, System.currentTimeMillis() - decoded.timestampMs());
            }
            return -1;
        }

        // Anti-replay: check if this token was already used
        ByteArrayKey tokenKey = new ByteArrayKey(tokenBytes);
        Long existingUsage = usedTokens.putIfAbsent(tokenKey, System.currentTimeMillis());
        if (existingUsage != null) {
            tokensReplayed.incrementAndGet();
            if (logger.isDebugEnabled()) {
                logger.debug("Retry token replay detected for address {}", address);
            }
            return -1;
        }

        // Evict old entries if we are at the capacity limit
        if (usedTokens.size() > MAX_USED_TOKENS) {
            evictOldUsedTokens();
        }

        tokensValidated.incrementAndGet();
        if (logger.isDebugEnabled()) {
            logger.debug("Retry token validated for address {} (original DCID len: {})",
                    address, decoded.originalDcid().length);
        }

        return tokenLen;
    }

    @Override
    public int maxTokenLength() {
        return codec.maxTokenLength();
    }

    /**
     * Returns the underlying codec for direct access (e.g., extracting original DCID).
     */
    public RetryTokenCodec codec() {
        return codec;
    }

    public long tokensIssued() {
        return tokensIssued.get();
    }

    public long tokensValidated() {
        return tokensValidated.get();
    }

    public long tokensRejected() {
        return tokensRejected.get();
    }

    public long tokensExpired() {
        return tokensExpired.get();
    }

    public long tokensReplayed() {
        return tokensReplayed.get();
    }

    /**
     * Evicts used token entries when the tracking map exceeds capacity.
     * Removes entries older than the token max age. If still over limit,
     * evicts entries from the oldest half of the token age window.
     */
    private void evictOldUsedTokens() {
        long now = System.currentTimeMillis();
        // First pass: remove entries older than tokenMaxAgeMs (they can't be replayed anyway)
        usedTokens.entrySet().removeIf(e -> now - e.getValue() > tokenMaxAgeMs);

        // If still over limit, evict entries from the older half of the valid window.
        // This is more correct than removing by iterator order (which is hash-order,
        // not chronological) and avoids O(n log n) sorting.
        if (usedTokens.size() > MAX_USED_TOKENS) {
            long midpointMs = now - (tokenMaxAgeMs / 2);
            usedTokens.entrySet().removeIf(e -> e.getValue() < midpointMs);
        }
    }
}
