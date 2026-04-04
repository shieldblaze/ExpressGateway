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
package com.shieldblaze.expressgateway.configuration.autoscaling;

import com.fasterxml.jackson.annotation.JsonProperty;
import com.shieldblaze.expressgateway.configuration.Configuration;
import lombok.Getter;
import lombok.Setter;
import lombok.ToString;
import lombok.experimental.Accessors;

import static com.shieldblaze.expressgateway.common.utils.NumberUtil.checkInRange;
import static com.shieldblaze.expressgateway.common.utils.NumberUtil.checkPositive;

@Getter
@Setter
@Accessors(fluent = true, chain = true)
@ToString
public final class AutoscalingConfiguration implements Configuration<AutoscalingConfiguration> {

    public static final AutoscalingConfiguration DEFAULT = new AutoscalingConfiguration();

    static {
        DEFAULT.cpuIsolateLoad = 0.70f;
        DEFAULT.cpuScaleOutLoad = 0.85f;

        DEFAULT.memoryIsolateLoad = 0.70f;
        DEFAULT.memoryScaleOutLoad = 0.85f;

        DEFAULT.packetsIsolateLoad = 0.70f;
        DEFAULT.packetsScaleOutLoad = 0.85f;
        DEFAULT.maxPackets = Long.MAX_VALUE;

        DEFAULT.bytesIsolateLoad = 0.70f;
        DEFAULT.bytesScaleOutLoad = 0.85f;
        DEFAULT.maxBytes = Long.MAX_VALUE;

    }

    /**
     * CPU Scale out load
     */
    @JsonProperty
    private float cpuScaleOutLoad;

    /**
     * CPU Isolate load
     */
    @JsonProperty
    private float cpuIsolateLoad;

    /**
     * Memory Scale out load
     */
    @JsonProperty
    private float memoryScaleOutLoad;

    /**
     * Memory Isolate load
     */
    @JsonProperty
    private float memoryIsolateLoad;

    /**
     * Packets Scale out load
     */
    @JsonProperty
    private float packetsScaleOutLoad;

    /**
     * Packets Isolate load
     */
    @JsonProperty
    private float packetsIsolateLoad;

    /**
     * Maximum Packets Count
     */
    @JsonProperty
    private long maxPackets;

    /**
     * Bytes Scale out load
     */
    @JsonProperty
    private float bytesScaleOutLoad;

    /**
     * Bytes Isolate load
     */
    @JsonProperty
    private float bytesIsolateLoad;

    /**
     * Maximum Bytes Count
     */
    @JsonProperty
    private long maxBytes;

    /**
     * Minimum number of Servers in fleet
     */
    @JsonProperty
    private int minServers;

    /**
     * Maximum number of Server to be autoscaled in fleet
     */
    @JsonProperty
    private int maxServers;

    /**
     * Scale out multiplier
     */
    @JsonProperty
    private int scaleOutMultiplier;

    /**
     * Isolation warmup time
     */
    @JsonProperty
    private int isolationWarmupTime;

    /**
     * Cooldown time in seconds of autoscaled servers
     */
    @JsonProperty
    private int coolDownTime;

    /**
     * If load is under certain threshold then we'll shut down the autoscaled server.
     * <p>
     * This works in combination with {@link #shutdownIfLoadUnderForSeconds}
     */
    @JsonProperty
    private float shutdownIfLoadUnder;

    /**
     * If load is under certain threshold for certain number of seconds then we'll shut down the autoscaled server.
     * <p>
     * This works in combination with {@link #shutdownIfLoadUnder}
     */
    @JsonProperty
    private int shutdownIfLoadUnderForSeconds;

    @Override
    public AutoscalingConfiguration validate() {
        checkInRange(cpuScaleOutLoad, 0.1f, 1.0f, "CPU Scale Out Load");
        checkInRange(cpuIsolateLoad, 0.1f, 1.0f, "CPU Isolate Load");
        checkInRange(memoryScaleOutLoad, 0.1f, 1.0f, "Memory Scale Out Load");
        checkInRange(memoryIsolateLoad, 0.1f, 1.0f, "Memory Isolate Load");
        checkInRange(packetsScaleOutLoad, 0.1f, 1.0f, "Packets Scale Out Load");
        checkInRange(packetsIsolateLoad, 0.1f, 1.0f, "Packets Isolate Load");
        checkPositive(maxPackets, "Maximum Packets");
        checkInRange(bytesScaleOutLoad, 0.1f, 1.0f, "Bytes Scale Out Load");
        checkInRange(bytesIsolateLoad, 0.1f, 1.0f, "Bytes Isolate Load");
        checkPositive(maxBytes, "Maximum Bytes");
        return this;
    }
}
