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
 * Listener for lifecycle phase transitions. Register with {@link LifecycleManager}
 * to receive notifications when phases change.
 */
@FunctionalInterface
public interface LifecycleListener {

    /**
     * Called when a lifecycle phase transition occurs.
     *
     * <p>This method is called synchronously during the phase transition.
     * Implementations should avoid blocking or performing heavy computation.
     * Exceptions thrown from this method are logged but do not prevent the transition.</p>
     *
     * @param event the lifecycle event describing the transition
     */
    void onPhaseTransition(LifecycleEvent event);
}
