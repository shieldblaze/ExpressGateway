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

import java.time.Duration;

/**
 * Record representing a lifecycle phase transition event.
 *
 * @param phase         the phase being entered
 * @param previousPhase the phase being left (null for the initial INITIALIZE transition)
 * @param componentName the name of the component involved (null for phase-level transitions)
 * @param duration      how long the transition took
 */
public record LifecycleEvent(
        LifecyclePhase phase,
        LifecyclePhase previousPhase,
        String componentName,
        Duration duration
) {
}
