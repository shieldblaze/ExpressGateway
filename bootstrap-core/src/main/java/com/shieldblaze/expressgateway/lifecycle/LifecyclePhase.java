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
package com.shieldblaze.expressgateway.lifecycle;

/**
 * Deterministic startup and shutdown phases for the gateway lifecycle.
 *
 * <p>Startup progresses forward through these phases in order:
 * {@code INITIALIZE -> CONSTRUCT -> CONNECT -> START -> RUNNING}.</p>
 *
 * <p>Shutdown progresses through:
 * {@code RUNNING -> DRAIN -> SHUTDOWN}.</p>
 *
 * <p>Each phase transition is logged with timing information. If any phase
 * fails during startup, already-started components are shut down in reverse order.</p>
 */
public enum LifecyclePhase {

    /**
     * Load and validate configuration. The gateway is not yet operational.
     */
    INITIALIZE,

    /**
     * Build the dependency graph and create components. No network connections yet.
     */
    CONSTRUCT,

    /**
     * Initialize coordination provider connection (if clustered mode).
     */
    CONNECT,

    /**
     * Start all registered lifecycle participants in priority order.
     */
    START,

    /**
     * The gateway is fully operational and serving traffic.
     */
    RUNNING,

    /**
     * Stop accepting new work; drain in-flight requests. Graceful shutdown begins.
     */
    DRAIN,

    /**
     * Close all components and release resources. Terminal state.
     */
    SHUTDOWN
}
