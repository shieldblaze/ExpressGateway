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
package com.shieldblaze.expressgateway.controlplane.registry;

/**
 * Lifecycle states for a data-plane node as tracked by the control plane.
 *
 * <p>State transitions:
 * <pre>
 *   CONNECTED --heartbeat--> HEALTHY --miss >= threshold--> UNHEALTHY --miss >= disconnect--> DISCONNECTED
 *        |                      |
 *        +--- drain command --->+-----> DRAINING ----> DISCONNECTED
 * </pre>
 */
public enum DataPlaneNodeState {

    /** Just connected, not yet healthy. Awaiting first successful heartbeat. */
    CONNECTED,

    /** Receiving heartbeats, configuration applied. Node is serving traffic. */
    HEALTHY,

    /** Missed heartbeats beyond the miss threshold. Node may be degraded. */
    UNHEALTHY,

    /** Draining connections, no new traffic should be routed to this node. */
    DRAINING,

    /** Lost connection. Node is no longer reachable and should be cleaned up. */
    DISCONNECTED
}
