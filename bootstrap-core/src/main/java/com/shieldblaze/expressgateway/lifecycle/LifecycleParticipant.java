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
 * Interface for components that participate in the gateway lifecycle.
 *
 * <p>Components register with the {@link LifecycleManager} and are started/stopped
 * according to their {@link #priority()} and {@link #phase()}.
 * Lower priority values start earlier and stop later.</p>
 *
 * <p>Implementations should be designed for clean startup and shutdown.
 * The {@link #stop()} method must be idempotent -- calling it multiple times
 * must not throw or cause side effects.</p>
 */
public interface LifecycleParticipant {

    /**
     * Returns a human-readable name for this participant, used in logging and diagnostics.
     *
     * @return the participant name (must not be null or blank)
     */
    String name();

    /**
     * Returns the startup priority. Lower values start earlier during startup
     * and stop later during shutdown (reverse order).
     *
     * <p>Default is 1000. Framework-level components (coordination, service discovery)
     * should use values below 500. Application-level components should use 1000+.</p>
     *
     * @return the priority value
     */
    default int priority() {
        return 1000;
    }

    /**
     * Starts this participant. Called during the startup sequence.
     *
     * @throws Exception if startup fails; the lifecycle manager will initiate rollback
     */
    void start() throws Exception;

    /**
     * Stops this participant. Called during shutdown in reverse priority order.
     * Must be idempotent.
     *
     * @throws Exception if shutdown encounters an error (logged but does not prevent other stops)
     */
    void stop() throws Exception;

    /**
     * Returns the lifecycle phase during which this participant should be started.
     * Participants are grouped by phase, then sorted by priority within each phase.
     *
     * @return the lifecycle phase (defaults to {@link LifecyclePhase#START})
     */
    default LifecyclePhase phase() {
        return LifecyclePhase.START;
    }
}
