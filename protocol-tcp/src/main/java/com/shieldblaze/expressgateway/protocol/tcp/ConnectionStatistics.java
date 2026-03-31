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
package com.shieldblaze.expressgateway.protocol.tcp;

import java.time.Instant;
import java.util.concurrent.atomic.AtomicLong;
import java.util.concurrent.atomic.LongAdder;

/**
 * Lock-free connection statistics tracker for TCP proxy sessions.
 *
 * <p>Uses {@link LongAdder} for high-throughput counters (bytes, messages) since
 * multiple EventLoop threads may update them concurrently. LongAdder avoids the
 * CAS contention of AtomicLong under high write rates by striping across cells.</p>
 *
 * <p>Uses {@link AtomicLong} for timestamps where CAS contention is negligible
 * (updated once per event, not per byte).</p>
 */
final class ConnectionStatistics {

    private final Instant createdAt;
    private final LongAdder bytesRead = new LongAdder();
    private final LongAdder bytesWritten = new LongAdder();
    private final LongAdder messagesRead = new LongAdder();
    private final LongAdder messagesWritten = new LongAdder();
    private final AtomicLong lastActivityNanos = new AtomicLong(System.nanoTime());
    private final LongAdder backpressurePauses = new LongAdder();

    ConnectionStatistics() {
        this.createdAt = Instant.now();
    }

    void recordBytesRead(long bytes) {
        bytesRead.add(bytes);
        messagesRead.increment();
        lastActivityNanos.set(System.nanoTime());
    }

    void recordBytesWritten(long bytes) {
        bytesWritten.add(bytes);
        messagesWritten.increment();
        lastActivityNanos.set(System.nanoTime());
    }

    void recordBackpressurePause() {
        backpressurePauses.increment();
    }

    /**
     * Snapshot the current statistics into an immutable record.
     * The snapshot is consistent at the instant of the call but individual
     * counters may advance between reads -- this is acceptable for monitoring.
     */
    Snapshot snapshot() {
        return new Snapshot(
                createdAt,
                bytesRead.sum(),
                bytesWritten.sum(),
                messagesRead.sum(),
                messagesWritten.sum(),
                System.nanoTime() - lastActivityNanos.get(),
                backpressurePauses.sum()
        );
    }

    /**
     * Immutable point-in-time snapshot of connection statistics.
     *
     * @param createdAt             when the connection was established
     * @param bytesRead             total bytes read from this side
     * @param bytesWritten          total bytes written to this side
     * @param messagesRead          total messages (channelRead calls) read
     * @param messagesWritten       total messages written
     * @param idleNanos             nanoseconds since last activity
     * @param backpressurePauses    number of times autoRead was toggled off due to backpressure
     */
    record Snapshot(
            Instant createdAt,
            long bytesRead,
            long bytesWritten,
            long messagesRead,
            long messagesWritten,
            long idleNanos,
            long backpressurePauses
    ) {
        /**
         * Duration this connection has been alive in milliseconds.
         */
        long uptimeMillis() {
            return java.time.Duration.between(createdAt, Instant.now()).toMillis();
        }

        /**
         * Total bytes transferred in both directions.
         */
        long totalBytes() {
            return bytesRead + bytesWritten;
        }
    }
}
