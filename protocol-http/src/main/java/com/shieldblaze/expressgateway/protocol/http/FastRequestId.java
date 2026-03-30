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

import java.util.concurrent.ThreadLocalRandom;

/**
 * ME-02: Fast, allocation-efficient request ID generator that produces
 * RFC 4122 UUID v4 formatted strings without {@code UUID.randomUUID()}.
 *
 * <p>{@code UUID.randomUUID()} uses {@code SecureRandom}, which on Linux
 * reads from {@code /dev/urandom} and has internal synchronization. Under
 * high concurrency, this becomes a contention bottleneck. Since request IDs
 * only need uniqueness (not cryptographic security), {@link ThreadLocalRandom}
 * is a suitable replacement with zero contention.</p>
 *
 * <p>Output format: UUID v4 (8-4-4-4-12 hex with version 4 and variant bits).
 * Example: {@code "a1b2c3d4-e5f6-4ab8-89d0-e1f2a3b4c5d6"}</p>
 */
final class FastRequestId {

    private static final char[] HEX = "0123456789abcdef".toCharArray();

    // Thread-local StringBuilder to avoid per-request allocation.
    // Capacity 36 = UUID with dashes (8-4-4-4-12).
    private static final ThreadLocal<StringBuilder> BUFFER = ThreadLocal.withInitial(() -> new StringBuilder(36));

    private FastRequestId() {}

    /**
     * Generate a UUID v4 formatted request ID.
     * Uses {@link ThreadLocalRandom} — zero contention, no allocation beyond
     * the thread-local StringBuilder reuse.
     *
     * <p>Sets version bits (0100) in bits 48-51 and variant bits (10) in
     * bits 64-65 per RFC 4122 Section 4.4.</p>
     */
    static String generate() {
        ThreadLocalRandom random = ThreadLocalRandom.current();
        long hi = random.nextLong();
        long lo = random.nextLong();

        // Set version 4: bits 48-51 = 0100
        hi = (hi & 0xFFFFFFFFFFFF0FFFL) | 0x0000000000004000L;
        // Set variant 1: bits 64-65 = 10
        lo = (lo & 0x3FFFFFFFFFFFFFFFL) | 0x8000000000000000L;

        StringBuilder sb = BUFFER.get();
        sb.setLength(0);
        // 8-4-4-4-12 format
        appendHex(sb, hi >>> 32, 8);
        sb.append('-');
        appendHex(sb, hi >>> 16, 4);
        sb.append('-');
        appendHex(sb, hi, 4);
        sb.append('-');
        appendHex(sb, lo >>> 48, 4);
        sb.append('-');
        appendHex(sb, lo, 12);
        return sb.toString();
    }

    private static void appendHex(StringBuilder sb, long value, int digits) {
        for (int i = (digits - 1) * 4; i >= 0; i -= 4) {
            sb.append(HEX[(int) ((value >>> i) & 0xF)]);
        }
    }
}
