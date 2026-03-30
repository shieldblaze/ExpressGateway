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
package com.shieldblaze.expressgateway.testing;

import java.time.Duration;
import java.util.Arrays;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.atomic.AtomicLong;
import java.util.concurrent.atomic.LongAdder;

/**
 * Lightweight performance test harness for benchmarking operations in tests.
 * Captures throughput and latency statistics (min, max, p50, p99, p999) using
 * virtual threads for concurrent execution.
 *
 * <p>This is NOT a replacement for JMH; it is designed for quick in-test
 * performance sanity checks and regression detection.</p>
 *
 * <p>Example:</p>
 * <pre>
 *   PerformanceTestHarness.Result result = PerformanceTestHarness.builder()
 *       .concurrency(10)
 *       .iterations(10_000)
 *       .warmupIterations(1_000)
 *       .operation(() -> httpClient.send(request, BodyHandlers.discarding()))
 *       .run();
 *
 *   System.out.println(result);
 *   assertTrue(result.p99Nanos() < Duration.ofMillis(50).toNanos());
 * </pre>
 */
public final class PerformanceTestHarness {

    private PerformanceTestHarness() {
    }

    /**
     * Result of a performance test run.
     */
    public record Result(
            long totalIterations,
            long totalDurationNanos,
            long minNanos,
            long maxNanos,
            long p50Nanos,
            long p99Nanos,
            long p999Nanos,
            long errorCount,
            double throughputOpsPerSec
    ) {
        @Override
        public String toString() {
            return String.format(
                    "PerformanceResult{iterations=%d, duration=%dms, throughput=%.1f ops/s, " +
                            "min=%dus, p50=%dus, p99=%dus, p999=%dus, max=%dus, errors=%d}",
                    totalIterations, totalDurationNanos / 1_000_000,
                    throughputOpsPerSec,
                    minNanos / 1_000, p50Nanos / 1_000, p99Nanos / 1_000,
                    p999Nanos / 1_000, maxNanos / 1_000, errorCount);
        }
    }

    /**
     * An operation to benchmark. May throw any exception.
     */
    @FunctionalInterface
    public interface BenchmarkOperation {
        void execute() throws Exception;
    }

    public static Builder builder() {
        return new Builder();
    }

    public static final class Builder {
        private int concurrency = 1;
        private int iterations = 1_000;
        private int warmupIterations = 100;
        private BenchmarkOperation operation;

        private Builder() {
        }

        /**
         * Number of concurrent virtual threads executing the operation. Default: 1.
         */
        public Builder concurrency(int concurrency) {
            if (concurrency < 1) {
                throw new IllegalArgumentException("concurrency must be >= 1");
            }
            this.concurrency = concurrency;
            return this;
        }

        /**
         * Total number of iterations (shared across all threads). Default: 1000.
         */
        public Builder iterations(int iterations) {
            if (iterations < 1) {
                throw new IllegalArgumentException("iterations must be >= 1");
            }
            this.iterations = iterations;
            return this;
        }

        /**
         * Warmup iterations (discarded from measurement). Default: 100.
         */
        public Builder warmupIterations(int warmupIterations) {
            if (warmupIterations < 0) {
                throw new IllegalArgumentException("warmupIterations must be >= 0");
            }
            this.warmupIterations = warmupIterations;
            return this;
        }

        /**
         * The operation to benchmark.
         */
        public Builder operation(BenchmarkOperation operation) {
            this.operation = operation;
            return this;
        }

        /**
         * Execute the benchmark and return results.
         */
        public Result run() throws InterruptedException {
            if (operation == null) {
                throw new IllegalStateException("operation must be set");
            }

            // Warmup phase
            for (int i = 0; i < warmupIterations; i++) {
                try {
                    operation.execute();
                } catch (Exception ignored) {
                    // Warmup errors are ignored
                }
            }

            // Measurement phase
            long[] latencies = new long[iterations];
            AtomicLong index = new AtomicLong(0);
            LongAdder errors = new LongAdder();
            CountDownLatch latch = new CountDownLatch(concurrency);

            long startTime = System.nanoTime();

            for (int t = 0; t < concurrency; t++) {
                Thread.ofVirtual().name("perf-worker-" + t).start(() -> {
                    try {
                        long idx;
                        while ((idx = index.getAndIncrement()) < iterations) {
                            long opStart = System.nanoTime();
                            try {
                                operation.execute();
                            } catch (Exception e) {
                                errors.increment();
                            }
                            latencies[(int) idx] = System.nanoTime() - opStart;
                        }
                    } finally {
                        latch.countDown();
                    }
                });
            }

            latch.await();
            long totalDuration = System.nanoTime() - startTime;

            // Compute percentiles
            int actualCount = (int) Math.min(index.get(), iterations);
            long[] measured = Arrays.copyOf(latencies, actualCount);
            Arrays.sort(measured);

            long minNanos = measured.length > 0 ? measured[0] : 0;
            long maxNanos = measured.length > 0 ? measured[measured.length - 1] : 0;
            long p50 = percentile(measured, 0.50);
            long p99 = percentile(measured, 0.99);
            long p999 = percentile(measured, 0.999);

            double throughput = actualCount * 1_000_000_000.0 / totalDuration;

            return new Result(actualCount, totalDuration, minNanos, maxNanos, p50, p99, p999,
                    errors.sum(), throughput);
        }

        private static long percentile(long[] sorted, double p) {
            if (sorted.length == 0) return 0;
            int idx = (int) Math.ceil(p * sorted.length) - 1;
            return sorted[Math.max(0, Math.min(idx, sorted.length - 1))];
        }
    }
}
