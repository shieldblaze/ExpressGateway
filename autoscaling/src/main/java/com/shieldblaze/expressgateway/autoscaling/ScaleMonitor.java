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

import com.shieldblaze.expressgateway.autoscaling.event.AutoscalingDehibernateEvent;
import com.shieldblaze.expressgateway.autoscaling.event.AutoscalingHibernateEvent;
import com.shieldblaze.expressgateway.autoscaling.event.AutoscalingScaleInEvent;
import com.shieldblaze.expressgateway.autoscaling.event.AutoscalingScaleOutEvent;
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
            double cpuLoad = cpu.cpu();
            if (cpuLoad > autoscaling.configuration().cpuScaleOutLoad()) {
                scaleOut();
            } else {
                scaleIn();
            }

            if (cpuLoad > autoscaling.configuration().cpuHibernateLoad()) {
                hibernate();
            } else {
                dehibernate();
            }

            // --------------------------------------- Memory ---------------------------------------
            float memoryLoad = memory.memory().physicalMemoryUsed();
            if (memoryLoad > autoscaling.configuration().memoryScaleOutLoad()) {
                scaleOut();
            } else {
                scaleIn();
            }

            if (memoryLoad > autoscaling.configuration().memoryHibernateLoad()) {
                hibernate();
            } else {
                scaleIn();
            }

            // --------------------------------------- Packets ---------------------------------------
            int rxPck = packetsMap.stream().mapToInt(Packets::rx).sum();
            int txPck = packetsMap.stream().mapToInt(Packets::tx).sum();
            float rxPckLoad = MathUtil.percentage(rxPck, autoscaling.configuration().maxPacketsPerSecond());
            float txPckLoad = MathUtil.percentage(txPck, autoscaling.configuration().maxPacketsPerSecond());

            if (rxPckLoad > autoscaling.configuration().packetsScaleOutLoad()) {
                scaleOut();
            } else {
                scaleIn();
            }

            if (txPckLoad > autoscaling.configuration().packetsScaleOutLoad()) {
                scaleOut();
            } else {
                scaleIn();
            }

            if (rxPckLoad > autoscaling.configuration().packetsHibernateLoad()) {
                hibernate();
            } else {
                dehibernate();
            }

            if (txPckLoad > autoscaling.configuration().packetsHibernateLoad()) {
                hibernate();
            } else {
                dehibernate();
            }

            // --------------------------------------- Bandwidth ---------------------------------------
            long rxBandwidth = bandwidthMap.stream().mapToLong(Bandwidth::rx).sum();
            long txBandwidth = bandwidthMap.stream().mapToLong(Bandwidth::tx).sum();
            float rxBandwidthLoad = MathUtil.percentage(rxBandwidth, autoscaling.configuration().maxBytesPerSecond());
            float txBandwidthLoad = MathUtil.percentage(txBandwidth, autoscaling.configuration().maxBytesPerSecond());

            if (rxBandwidthLoad > autoscaling.configuration().bytesScaleOutLoad()) {
                scaleOut();
            } else {
                scaleIn();
            }

            if (txBandwidthLoad > autoscaling.configuration().bytesScaleOutLoad()) {
                scaleOut();
            } else {
                scaleIn();
            }

            if (rxBandwidthLoad > autoscaling.configuration().bytesHibernateLoad()) {
                hibernate();
            } else {
                dehibernate();
            }

            if (txBandwidthLoad > autoscaling.configuration().bytesHibernateLoad()) {
                hibernate();
            } else {
                dehibernate();
            }
        } catch (IOException e) {
            // This should never happen
            Error error = new Error(e);
            logger.fatal(error);
        }
    }

    private void scaleOut() {
        // Only ScaleOut if current state is NORMAL.
        if (autoscaling.server().state() == State.NORMAL) {
            autoscaling.server().state(State.COOLDOWN);

            AutoscalingScaleOutEvent scaleOutEvent = autoscaling.scaleOut();
            autoscaling.eventStream().publish(scaleOutEvent);
        }
    }

    private void scaleIn() {
        if (autoscaling.server().state() == State.COOLDOWN) {
            autoscaling.server().state(State.NORMAL);

            AutoscalingScaleInEvent scaleInEvent = autoscaling.scaleIn();
            autoscaling.eventStream().publish(scaleInEvent);
        }
    }

    private void hibernate() {
        if (autoscaling.server().state() == State.COOLDOWN) {
            autoscaling.server().state(State.HIBERNATE);

            AutoscalingHibernateEvent hibernateEvent = autoscaling.hibernate();
            autoscaling.eventStream().publish(hibernateEvent);
        }
    }

    private void dehibernate() {
        if (autoscaling.server().state() == State.HIBERNATE) {
            autoscaling.server().state(State.COOLDOWN);

            AutoscalingDehibernateEvent dehibernateEvent = autoscaling.dehibernate();
            autoscaling.eventStream().publish(dehibernateEvent);
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
