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

import com.shieldblaze.expressgateway.controlplane.registry.DataPlaneNode;

import java.util.List;

/**
 * Strategy for distributing config updates to data-plane nodes.
 *
 * <p>Implementations define the topology and transport used for push-based
 * config distribution. The control plane selects a strategy based on fleet
 * size and network topology:</p>
 * <ul>
 *   <li>{@link DirectFanOut} -- flat CP-to-all-nodes, suitable up to ~10K nodes</li>
 * </ul>
 */
public interface FanOutStrategy {

    /**
     * Distribute a config delta to the specified target nodes.
     *
     * <p>Implementations must be resilient to individual node failures:
     * a push failure to one node must not prevent delivery to the remaining targets.
     * Nodes that NACK or fail will be retried on the next push cycle.</p>
     *
     * @param delta   the config changes to distribute; must not be null or empty
     * @param targets the nodes to receive the update; must not be null
     */
    void distribute(ConfigDelta delta, List<DataPlaneNode> targets);
}
