/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2021 ShieldBlaze
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

import com.shieldblaze.expressgateway.autoscaling.event.AutoscalingScaleDownEvent;
import com.shieldblaze.expressgateway.autoscaling.event.AutoscalingScaleUpEvent;
import com.shieldblaze.expressgateway.common.utils.NetworkInterfaceUtil;
import com.shieldblaze.expressgateway.configuration.autoscaling.AutoscalingConfiguration;
import com.shieldblaze.expressgateway.metrics.Bandwidth;
import com.shieldblaze.expressgateway.metrics.CPU;
import com.shieldblaze.expressgateway.metrics.Memory;
import com.shieldblaze.expressgateway.metrics.Packets;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.io.Closeable;
import java.io.IOException;
import java.util.List;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.TimeUnit;

/**
 * {@link ScaleUpMonitor} monitors CPU, Memory, Packets and Bandwidth load
 * and triggers scale up when necessary.
 */
public final class ScaleUpMonitor implements Runnable, Closeable {

    private static final Logger logger = LogManager.getLogger(ScaleUpMonitor.class);

    private final ScheduledExecutorService scheduledExecutorService = Executors.newSingleThreadScheduledExecutor();
    private final ScheduledFuture<?> monitorScheduledFuture;
    private final ScheduledFuture<?> packetsAndBandwidthMonitorScheduledFuture;

    private final CPU cpu = new CPU();
    private final Memory memory = new Memory();
    private final List<Packets> packetsMap = new CopyOnWriteArrayList<>();
    private final List<Bandwidth> bandwidthMap = new CopyOnWriteArrayList<>();

    private final AutoscalingConfiguration autoscalingConfiguration;
    private final Autoscaling autoscaling;

    /**
     * Create a new {@link ScaleUpMonitor} Instance
     *
     * @param autoscalingConfiguration {@link AutoscalingConfiguration} Instance
     */
    public ScaleUpMonitor(AutoscalingConfiguration autoscalingConfiguration, Autoscaling autoscaling) {
        this.autoscalingConfiguration = autoscalingConfiguration;
        this.autoscaling = autoscaling;

        // Schedule this Runnable to execute every second.
        monitorScheduledFuture = scheduledExecutorService.scheduleAtFixedRate(this, 0, 1, TimeUnit.SECONDS);
        packetsAndBandwidthMonitorScheduledFuture = scheduledExecutorService.scheduleAtFixedRate(new PacketsAndBandwidthMonitor(), 0, 1, TimeUnit.MINUTES);
    }

    @Override
    public void run() {
        try {
            // --------------------------------------- CPU ---------------------------------------
            double cpuLoad = cpu.cpu();
            if (cpuLoad > autoscalingConfiguration.maxCPULoad()) {
                scaleUp();
            }

            // --------------------------------------- Memory ---------------------------------------
            float memoryLoad = memory.memory().physicalMemoryUsed();
            if (memoryLoad > autoscalingConfiguration.maxMemoryLoad()) {
                scaleUp();
            }

            // --------------------------------------- Packets ---------------------------------------
            int rxPck = packetsMap.stream().mapToInt(Packets::rx).sum();
            int txPck = packetsMap.stream().mapToInt(Packets::tx).sum();

            if (rxPck > autoscalingConfiguration.maxPacketsPerSecond()) {
                scaleUp();
            }

            if (txPck > autoscalingConfiguration.maxPacketsPerSecond()) {
                scaleUp();
            }

            // --------------------------------------- Bandwidth ---------------------------------------
            long rxBandwidth = bandwidthMap.stream().mapToLong(Bandwidth::rx).sum();
            long txBandwidth = bandwidthMap.stream().mapToLong(Bandwidth::tx).sum();

            if (rxBandwidth > autoscalingConfiguration.maxBytesPerSecond()) {
                scaleUp();
            }

            if (txBandwidth > autoscalingConfiguration.maxBytesPerSecond()) {
                scaleUp();
            }
        } catch (IOException e) {
            // This should never happen
            Error error = new Error(e);
            logger.fatal(error);
        }
    }

    private void scaleUp() {
        AutoscalingScaleUpEvent scaleUpEvent = autoscaling.scaleUp();
        autoscaling.eventStream().publish(scaleUpEvent);
    }

    private void scaleDown() {
        AutoscalingScaleDownEvent scaleDownEvent = autoscaling.scaleDown();
        autoscaling.eventStream().publish(scaleDownEvent);
    }

    private final class PacketsAndBandwidthMonitor implements Runnable {

        @Override
        public void run() {
            packetsMap.forEach(Packets::close);
            packetsMap.clear();

            bandwidthMap.forEach(Bandwidth::close);
            bandwidthMap.clear();

            List<String> ifNameList = NetworkInterfaceUtil.ifNameList();

            for (String ifName : ifNameList) {
                Packets packets = new Packets(ifName);
                packets.start();
                packetsMap.add(packets);

                Bandwidth bandwidth = new Bandwidth(ifName);
                bandwidth.start();
                bandwidthMap.add(bandwidth);
            }
        }
    }

    @Override
    public void close() {
        scheduledExecutorService.shutdown();
        monitorScheduledFuture.cancel(true);
        packetsAndBandwidthMonitorScheduledFuture.cancel(true);
    }
}
