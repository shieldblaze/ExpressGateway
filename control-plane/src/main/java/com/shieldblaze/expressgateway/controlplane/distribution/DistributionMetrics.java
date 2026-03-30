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
package com.shieldblaze.expressgateway.controlplane.distribution;

import io.micrometer.core.instrument.Counter;
import io.micrometer.core.instrument.DistributionSummary;
import io.micrometer.core.instrument.MeterRegistry;
import io.micrometer.core.instrument.Timer;

import java.util.Objects;

/**
 * Tracks config distribution metrics: push success/failure/NACK counts,
 * push latency, and batch sizes.
 *
 * <p>All metrics are prefixed with {@code expressgateway.controlplane.} and
 * registered against the provided {@link MeterRegistry}.</p>
 *
 * <p>Intended usage: wrap each push call with {@link #startTimer()} /
 * {@link #stopTimer(Timer.Sample)} and record the outcome via
 * {@link #recordSuccess()}, {@link #recordFailure()}, or {@link #recordNack()}.</p>
 */
public final class DistributionMetrics {

    private final MeterRegistry meterRegistry;
    private final Counter pushSuccessCounter;
    private final Counter pushFailureCounter;
    private final Counter pushNackCounter;
    private final Timer pushLatencyTimer;
    private final DistributionSummary batchSizeSummary;

    public DistributionMetrics(MeterRegistry meterRegistry) {
        this.meterRegistry = Objects.requireNonNull(meterRegistry, "meterRegistry");

        this.pushSuccessCounter = Counter.builder("expressgateway.controlplane.push.success")
                .description("Number of successful config pushes to data-plane nodes")
                .register(meterRegistry);

        this.pushFailureCounter = Counter.builder("expressgateway.controlplane.push.failure")
                .description("Number of failed config pushes (transport errors)")
                .register(meterRegistry);

        this.pushNackCounter = Counter.builder("expressgateway.controlplane.push.nack")
                .description("Number of NACKed config pushes (node rejected the delta)")
                .register(meterRegistry);

        this.pushLatencyTimer = Timer.builder("expressgateway.controlplane.push.latency")
                .description("Config push latency per node")
                .register(meterRegistry);

        this.batchSizeSummary = DistributionSummary.builder("expressgateway.controlplane.batch.size")
                .description("Number of mutations per flushed batch")
                .register(meterRegistry);
    }

    /** Record a successful push to a node. */
    public void recordSuccess() {
        pushSuccessCounter.increment();
    }

    /** Record a transport-level push failure. */
    public void recordFailure() {
        pushFailureCounter.increment();
    }

    /** Record a NACK from a node. */
    public void recordNack() {
        pushNackCounter.increment();
    }

    /** Start a latency timer sample. */
    public Timer.Sample startTimer() {
        return Timer.start(meterRegistry);
    }

    /** Stop a latency timer sample and record the duration. */
    public void stopTimer(Timer.Sample sample) {
        Objects.requireNonNull(sample, "sample");
        sample.stop(pushLatencyTimer);
    }

    /** Record the size of a flushed batch. */
    public void recordBatchSize(int size) {
        batchSizeSummary.record(size);
    }
}
