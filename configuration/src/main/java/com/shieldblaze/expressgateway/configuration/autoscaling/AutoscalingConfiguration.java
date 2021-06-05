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
package com.shieldblaze.expressgateway.configuration.autoscaling;

import com.shieldblaze.expressgateway.common.utils.NumberUtil;

public final class AutoscalingConfiguration {

    /**
     * CPU Scale out load
     */
    private float cpuScaleOutLoad;

    /**
     * CPU Hibernate load
     */
    private float cpuHibernateLoad;

    /**
     * Memory Scale out load
     */
    private float memoryScaleOutLoad;

    /**
     * Memory Hibernate load
     */
    private float memoryHibernateLoad;

    /**
     * Maximum Packets Per Second
     */
    private int maxPacketsPerSecond;

    /**
     * Packets Scale out load
     */
    private float packetsScaleOutLoad;

    /**
     * Packets Hibernate load
     */
    private float packetsHibernateLoad;

    /**
     * Maximum Bytes per Second
     */
    private int maxBytesPerSecond;

    /**
     * Bytes Scale out load
     */
    private float bytesScaleOutLoad;

    /**
     * Bytes Hibernate load
     */
    private float bytesHibernateLoad;

    /**
     * Minimum number of Servers in fleet
     */
    private int minServers;

    /**
     * Maximum number of Server to be autoscaled in fleet
     */
    private int maxServers;

    /**
     * Scale out multiplier
     */
    private int scaleOutMultiplier;

    /**
     * Cooldown time in seconds of autoscaled servers
     */
    private int cooldownTime;

    /**
     * If load is under certain threshold
     * then we'll shutdown the autoscaled server.
     *
     * This works in combination with {@link #shutdownIfLoadUnderForSeconds}
     */
    private float shutdownIfLoadUnder;

    /**
     * If load is under certain threshold
     * for certain number of seconds
     * then we'll shutdown the autoscaled server.
     *
     * This works in combination with {@link #shutdownIfLoadUnder}
     */
    private int shutdownIfLoadUnderForSeconds;

    public void setCpuScaleOutLoad(float cpuScaleOutLoad) {
        this.cpuScaleOutLoad = NumberUtil.checkRange(cpuScaleOutLoad, 0.1f, 1.0f, "CPUScaleOutLoad");
    }

    public void setCpuHibernateLoad(float cpuHibernateLoad) {
        this.cpuHibernateLoad = NumberUtil.checkRange(cpuHibernateLoad, 0.1f, 1.0f, "CPUHibernateLoad");
    }

    public void setMemoryScaleOutLoad(float memoryScaleOutLoad) {
        this.memoryScaleOutLoad = NumberUtil.checkRange(memoryScaleOutLoad, 0.1f, 1.0f, "MemoryScaleOutLoad");
    }

    public void setMemoryHibernateLoad(float memoryHibernateLoad) {
        this.memoryHibernateLoad = NumberUtil.checkRange(memoryHibernateLoad, 0.1f, 1.0f, "MemoryHibernateLoad");
    }

    public void setMaxPacketsPerSecond(int maxPacketsPerSecond) {
        this.maxPacketsPerSecond = NumberUtil.checkPositive(maxPacketsPerSecond, "MaxPacketsPerSecond");
    }

    public void setPacketsScaleOutLoad(float packetsScaleOutLoad) {
        this.packetsScaleOutLoad = NumberUtil.checkRange(packetsScaleOutLoad, 0.1f, 1.0f, "PacketsScaleOutLoad");
    }

    public void setPacketsHibernateLoad(float packetsHibernateLoad) {
        this.packetsHibernateLoad = NumberUtil.checkRange(packetsHibernateLoad, 0.1f, 1.0f, "PacketsHibernateLoad");
    }

    public void setMaxBytesPerSecond(int maxBytesPerSecond) {
        this.maxBytesPerSecond = NumberUtil.checkPositive(maxBytesPerSecond, "MaxBytesPerSecond");
    }

    public void setBytesScaleOutLoad(float bytesScaleOutLoad) {
        this.bytesScaleOutLoad = NumberUtil.checkRange(bytesScaleOutLoad, 0.1f, 1.0f, "BytesScaleOutLoad");
    }

    public void setBytesHibernateLoad(float bytesHibernateLoad) {
        this.bytesHibernateLoad = NumberUtil.checkRange(bytesHibernateLoad, 0.1f, 1.0f, "BytesHibernateLoad");
    }

    public void setMinServers(int minServers) {
        this.minServers = NumberUtil.checkPositive(minServers, "MinServers");
    }

    public void setMaxServers(int maxServers) {
        this.maxServers = NumberUtil.checkPositive(maxServers, "MaxServers");
    }

    public void setScaleOutMultiplier(int scaleOutMultiplier) {
        this.scaleOutMultiplier = NumberUtil.checkPositive(scaleOutMultiplier, "ScaleOutMultiplier");
    }

    public void setCooldownTime(int cooldownTime) {
        this.cooldownTime = NumberUtil.checkPositive(cooldownTime, "cooldownTime");
    }

    public void setShutdownIfLoadUnder(float shutdownIfLoadUnder) {
        this.shutdownIfLoadUnder = NumberUtil.checkRange(shutdownIfLoadUnder, 0.1f, 1.0f, "ShutdownIfLoadUnder");
    }

    public void setShutdownIfLoadUnderForSeconds(int shutdownIfLoadUnderForSeconds) {
        this.shutdownIfLoadUnderForSeconds = NumberUtil.checkPositive(shutdownIfLoadUnderForSeconds, "ShutdownIfLoadUnderForSeconds");
    }

    public float cpuScaleOutLoad() {
        return cpuScaleOutLoad;
    }

    public float cpuHibernateLoad() {
        return cpuHibernateLoad;
    }

    public float memoryScaleOutLoad() {
        return memoryScaleOutLoad;
    }

    public float memoryHibernateLoad() {
        return memoryHibernateLoad;
    }

    public int maxPacketsPerSecond() {
        return maxPacketsPerSecond;
    }

    public float packetsScaleOutLoad() {
        return packetsScaleOutLoad;
    }

    public float packetsHibernateLoad() {
        return packetsHibernateLoad;
    }

    public int maxBytesPerSecond() {
        return maxBytesPerSecond;
    }

    public float bytesScaleOutLoad() {
        return bytesScaleOutLoad;
    }

    public float bytesHibernateLoad() {
        return bytesHibernateLoad;
    }

    public int minServers() {
        return minServers;
    }

    public int maxServers() {
        return maxServers;
    }

    public int scaleOutMultiplier() {
        return scaleOutMultiplier;
    }

    public int cooldownTime() {
        return cooldownTime;
    }

    public float shutdownIfLoadUnder() {
        return shutdownIfLoadUnder;
    }

    public int shutdownIfLoadUnderForSeconds() {
        return shutdownIfLoadUnderForSeconds;
    }
}
