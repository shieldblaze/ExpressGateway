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
package com.shieldblaze.expressgateway.protocol.http.grpc;

import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.ScheduledFuture;

/**
 * Manages gRPC deadline parsing and scheduled deadline enforcement.
 *
 * <h3>Timeout Parsing</h3>
 * <p>Parses the gRPC-Timeout header value into nanoseconds. The format is a positive
 * integer followed by a single-character unit suffix:</p>
 * <ul>
 *   <li>{@code H} - hours</li>
 *   <li>{@code M} - minutes</li>
 *   <li>{@code S} - seconds</li>
 *   <li>{@code m} - milliseconds</li>
 *   <li>{@code u} - microseconds</li>
 *   <li>{@code n} - nanoseconds</li>
 * </ul>
 * <p>Example: "5S" = 5,000,000,000 nanoseconds, "100m" = 100,000,000 nanoseconds.</p>
 *
 * <h3>Deadline Lifecycle</h3>
 * <p>Scheduled deadline futures MUST be cancelled when a stream completes normally
 * (response received, RST_STREAM, GOAWAY). Failure to cancel leaks ScheduledFuture
 * objects in the EventLoop's task queue and in the deadline map, causing unbounded
 * memory growth under sustained traffic with long deadline values.</p>
 *
 * <p>Callers use {@link #cancelDeadline(ConcurrentHashMap, int)} on every stream
 * completion path: RST_STREAM, DATA endStream, GOAWAY stream pruning, and response
 * trailers endStream. The {@link #cancelAllDeadlines(ConcurrentHashMap)} method is
 * used for connection-level teardown.</p>
 */
public final class GrpcDeadlineHandler {

    private static final long NANOS_PER_HOUR = 3_600_000_000_000L;
    private static final long NANOS_PER_MINUTE = 60_000_000_000L;
    private static final long NANOS_PER_SECOND = 1_000_000_000L;
    private static final long NANOS_PER_MILLI = 1_000_000L;
    private static final long NANOS_PER_MICRO = 1_000L;

    /**
     * Parses a gRPC timeout value into nanoseconds.
     *
     * @param grpcTimeout the raw grpc-timeout header value (e.g. "5S", "100m")
     * @return the timeout in nanoseconds, or {@code -1} if the value is null, empty, or invalid
     */
    public static long parseTimeoutNanos(CharSequence grpcTimeout) {
        if (grpcTimeout == null) {
            return -1;
        }

        int length = grpcTimeout.length();
        if (length < 2) {
            return -1;
        }

        char unit = grpcTimeout.charAt(length - 1);
        long multiplier;
        switch (unit) {
            case 'H':
                multiplier = NANOS_PER_HOUR;
                break;
            case 'M':
                multiplier = NANOS_PER_MINUTE;
                break;
            case 'S':
                multiplier = NANOS_PER_SECOND;
                break;
            case 'm':
                multiplier = NANOS_PER_MILLI;
                break;
            case 'u':
                multiplier = NANOS_PER_MICRO;
                break;
            case 'n':
                multiplier = 1;
                break;
            default:
                return -1;
        }

        long value;
        try {
            value = Long.parseLong(grpcTimeout, 0, length - 1, 10);
        } catch (NumberFormatException e) {
            return -1;
        }

        if (value < 0) {
            return -1;
        }

        // Overflow check: if multiplier > 1, check that value * multiplier does not overflow
        if (multiplier > 1 && value > Long.MAX_VALUE / multiplier) {
            return Long.MAX_VALUE;
        }

        return value * multiplier;
    }

    /**
     * Cancels and removes the deadline future for a specific stream ID.
     *
     * <p>This MUST be called on every stream completion path to prevent
     * ScheduledFuture leaks:</p>
     * <ul>
     *   <li>RST_STREAM from client (stream cancellation)</li>
     *   <li>DATA frame with endStream (request body complete)</li>
     *   <li>GOAWAY stream pruning (streams above lastStreamId)</li>
     *   <li>Response completion (handled by deadline lambda's own cleanup)</li>
     * </ul>
     *
     * <p>Safe to call even if no deadline was scheduled for the given stream ID
     * (no-op in that case). {@code cancel(false)} is used to avoid interrupting
     * the EventLoop thread if the task is currently executing.</p>
     *
     * @param deadlineFutures the map of stream ID to scheduled deadline futures
     * @param streamId        the stream ID whose deadline should be cancelled
     */
    public static void cancelDeadline(ConcurrentHashMap<Integer, ScheduledFuture<?>> deadlineFutures,
                                      int streamId) {
        ScheduledFuture<?> future = deadlineFutures.remove(streamId);
        if (future != null) {
            future.cancel(false);
        }
    }

    /**
     * Cancels all pending deadline futures and clears the map.
     *
     * <p>Used during connection-level teardown (close, GOAWAY). After this call,
     * no deadline tasks will fire for any stream on this connection.</p>
     *
     * @param deadlineFutures the map of stream ID to scheduled deadline futures
     */
    public static void cancelAllDeadlines(ConcurrentHashMap<Integer, ScheduledFuture<?>> deadlineFutures) {
        deadlineFutures.values().forEach(future -> future.cancel(false));
        deadlineFutures.clear();
    }

    private GrpcDeadlineHandler() {
        // Prevent outside initialization
    }
}
