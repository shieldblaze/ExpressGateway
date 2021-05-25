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

public final class AutoscalingConfiguration {

    /**
     * Maximum CPU Load
     */
    private float maxCPULoad;

    /**
     * Maximum Memory Load
     */
    private float maxMemoryLoad;

    /**
     * Maximum Packets Per Second
     */
    private int maxPacketsPerSecond;

    /**
     * Maximum Bytes per Second
     */
    private int maxBytesPerSecond;

    /**
     * Minimum number of Servers in fleet
     */
    private int minServers;

    /**
     * Maximum number of Server to be autoscaled in fleet
     */
    private int maxServers;

    /**
     * Cooldown time in seconds of autoscaled servers
     */
    private int cooldownTime;

    /**
     * Autoscaled server will be shutdown if it's under certain load
     * of connections.
     */
    private float shutdownIfConnectionBelow;

    public float maxCPULoad() {
        return maxCPULoad;
    }

    public float maxMemoryLoad() {
        return maxMemoryLoad;
    }

    public int maxPacketsPerSecond() {
        return maxPacketsPerSecond;
    }

    public int maxBytesPerSecond() {
        return maxBytesPerSecond;
    }

    public int minServers() {
        return minServers;
    }

    public int maxServers() {
        return maxServers;
    }
}
