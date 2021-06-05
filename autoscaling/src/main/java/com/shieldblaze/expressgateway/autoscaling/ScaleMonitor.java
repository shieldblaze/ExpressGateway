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

import com.shieldblaze.expressgateway.common.utils.MathUtil;
import com.shieldblaze.expressgateway.common.utils.NetworkInterfaceUtil;
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
 * {@link ScaleMonitor} monitors CPU, Memory, Packets and Bandwidth load
 * and triggers scale in or out / hibernate or dehibernate when necessary.
 */
public final class ScaleMonitor implements Runnable, Closeable {

    private static final Logger logger = LogManager.getLogger(ScaleMonitor.class);

    private final ScheduledExecutorService scheduledExecutorService = Executors.newSingleThreadScheduledExecutor();
    private final ScheduledFuture<?> monitorScheduledFuture;
    private final ScheduledFuture<?> packetsAndBandwidthMonitorScheduledFuture;

    private final CPU cpu = new CPU();
    private final Memory memory = new Memory();
    private final List<Packets> packetsMap = new CopyOnWriteArrayList<>();
    private final List<Bandwidth> bandwidthMap = new CopyOnWriteArrayList<>();

    private final Autoscaling autoscaling;

    /**
     * Create a new {@link ScaleMonitor} Instance
     *
     * @param autoscaling {@link Autoscaling} Instance
     */
    public ScaleMonitor(Autoscaling autoscaling) {
        this.autoscaling = autoscaling;

        // Schedule this Runnable to execute every second.
        monitorScheduledFuture = scheduledExecutorService.scheduleAtFixedRate(this, 0, 1, TimeUnit.SECONDS);
        packetsAndBandwidthMonitorScheduledFuture = scheduledExecutorService.scheduleAtFixedRate(new PacketsAndBandwidthMonitor(), 0, 1, TimeUnit.MINUTES);
    }

    @Override
    public void run() {
        try {
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
            float memoryLoad = memory.memory().physicalMemoryUsed();
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
            int rxPck = packetsMap.stream().mapToInt(Packets::rx).sum();
            int txPck = packetsMap.stream().mapToInt(Packets::tx).sum();
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
            long rxBandwidth = bandwidthMap.stream().mapToLong(Bandwidth::rx).sum();
            long txBandwidth = bandwidthMap.stream().mapToLong(Bandwidth::tx).sum();
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
        } catch (IOException e) {
            // This should never happen
            Error error = new Error(e);
            logger.fatal(error);
        }
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
