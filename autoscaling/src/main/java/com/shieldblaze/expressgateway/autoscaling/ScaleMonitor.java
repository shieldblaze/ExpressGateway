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
package com.shieldblaze.expressgateway.autoscaling;

import com.shieldblaze.expressgateway.common.utils.MathUtil;
import com.shieldblaze.expressgateway.metrics.CPU;
import com.shieldblaze.expressgateway.metrics.CPUMetric;
import com.shieldblaze.expressgateway.metrics.EdgeNetworkMetric;
import com.shieldblaze.expressgateway.metrics.EdgeNetworkMetricRecorder;
import com.shieldblaze.expressgateway.metrics.Memory;
import com.shieldblaze.expressgateway.metrics.MemoryMetric;

import java.io.Closeable;
import java.util.Objects;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.TimeUnit;

/**
 * {@link ScaleMonitor} monitors CPU, Memory, Packets and Bandwidth load
 * and triggers scale in or out / hibernate or dehibernate when necessary.
 */
public final class ScaleMonitor implements Runnable, Closeable {

    private final ScheduledExecutorService scheduledExecutorService = Executors.newSingleThreadScheduledExecutor();
    private final ScheduledFuture<?> monitorScheduledFuture;

    private final CPUMetric cpu;
    private final MemoryMetric memory;
    private final EdgeNetworkMetric edgeNetworkMetric;

    private final Autoscaling autoscaling;

    /**
     * Create a new {@link ScaleMonitor} Instance
     *
     * @param autoscaling {@link Autoscaling} Instance
     */
    public ScaleMonitor(Autoscaling autoscaling, CPUMetric cpu, MemoryMetric memory, EdgeNetworkMetric edgeNetworkMetric) {
        this.autoscaling = autoscaling;

        this.cpu = Objects.requireNonNullElseGet(cpu, CPU::new);
        this.memory = Objects.requireNonNullElseGet(memory, Memory::new);
        this.edgeNetworkMetric = Objects.requireNonNullElse(edgeNetworkMetric, EdgeNetworkMetricRecorder.INSTANCE);

        monitorScheduledFuture = scheduledExecutorService.scheduleAtFixedRate(this, 0, 1, TimeUnit.SECONDS);
    }

    @Override
    public void run() {
        // --------------------------------------- CPU ---------------------------------------

        /*
         * If CPU load has exceeded normal level CPU load then scale out.
         * If not, then scale in.
         */
        double cpuLoad = cpu.cpu();
        if (cpuLoad > autoscaling.configuration().cpuScaleOutLoad()) {
            autoscaling.scaleOut();
        } else {
            autoscaling.scaleIn();
        }

        /*
         * If CPU load has exceeded hibernate level CPU load then enter in hibernation mode.
         * If not, then dehibernate.
         */
        if (cpuLoad > autoscaling.configuration().cpuHibernateLoad()) {
            autoscaling.hibernate();
        } else {
            autoscaling.dehibernate();
        }

        // --------------------------------------- Memory ---------------------------------------

        /*
         * If Memory load has exceeded normal Memory level then scale out.
         * If not, then scale in.
         */
        float memoryLoad = memory.physicalMemoryUsed();
        if (memoryLoad > autoscaling.configuration().memoryScaleOutLoad()) {
            autoscaling.scaleOut();
        } else {
            autoscaling.scaleIn();
        }

        /*
         * If Memory load has exceeded hibernate Memory level then enter in hibernation mode.
         * If not, then dehibernate.
         */
        if (memoryLoad > autoscaling.configuration().memoryHibernateLoad()) {
            autoscaling.hibernate();
        } else {
            autoscaling.dehibernate();
        }

        // --------------------------------------- Packets ---------------------------------------
        int rxPck = edgeNetworkMetric.packetRX();
        int txPck = edgeNetworkMetric.packetTX();
        float rxPckLoad = MathUtil.percentage(rxPck, autoscaling.configuration().maxPacketsPerSecond());
        float txPckLoad = MathUtil.percentage(txPck, autoscaling.configuration().maxPacketsPerSecond());

        /*
         * If Packets load has exceeded normal Packets load level then scale out.
         * If not, then scale in.
         */
        if (rxPckLoad > autoscaling.configuration().packetsScaleOutLoad() || txPckLoad > autoscaling.configuration().packetsScaleOutLoad()) {
            autoscaling.scaleOut();
        } else {
            autoscaling.scaleIn();
        }

        /*
         * If Packets load has exceeded hibernate Packets level then enter in hibernation mode.
         * If not, then scale in.
         */
        if (rxPckLoad > autoscaling.configuration().packetsHibernateLoad() || txPckLoad > autoscaling.configuration().packetsHibernateLoad()) {
            autoscaling.hibernate();
        } else {
            autoscaling.dehibernate();
        }

        // --------------------------------------- Bandwidth ---------------------------------------
        long rxBandwidth = edgeNetworkMetric.bandwidthRX();
        long txBandwidth = edgeNetworkMetric.bandwidthTX();
        float rxBandwidthLoad = MathUtil.percentage(rxBandwidth, autoscaling.configuration().maxBytesPerSecond());
        float txBandwidthLoad = MathUtil.percentage(txBandwidth, autoscaling.configuration().maxBytesPerSecond());

        /*
         * If Bandwidth load has exceeded normal Bandwidth load level then scale out.
         * If not, then scale in.
         */
        if (rxBandwidthLoad > autoscaling.configuration().bytesScaleOutLoad() || txBandwidthLoad > autoscaling.configuration().bytesScaleOutLoad()) {
            autoscaling.scaleOut();
        } else {
            autoscaling.scaleIn();
        }

        /*
         * If Bandwidth load has exceeded hibernate Bandwidth level then enter in hibernation mode.
         * If not, then scale in.
         */
        if (rxBandwidthLoad > autoscaling.configuration().bytesHibernateLoad() || txBandwidthLoad > autoscaling.configuration().bytesHibernateLoad()) {
            autoscaling.hibernate();
        } else {
            autoscaling.dehibernate();
        }
    }

    @Override
    public void close() {
        monitorScheduledFuture.cancel(true);
        scheduledExecutorService.shutdown();
    }
}
