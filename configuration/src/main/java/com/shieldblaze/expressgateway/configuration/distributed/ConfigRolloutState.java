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
package com.shieldblaze.expressgateway.configuration.distributed;

/**
 * Represents the state of a configuration rollout in the distributed system.
 */
public enum ConfigRolloutState {

    /**
     * Configuration has been proposed but not yet validated or applied.
     */
    PROPOSED,

    /**
     * Configuration is currently being validated.
     */
    VALIDATING,

    /**
     * Configuration has been validated and applied successfully.
     */
    APPLIED,

    /**
     * Configuration has been confirmed by the cluster.
     */
    CONFIRMED,

    /**
     * Configuration was rolled back to a previous version.
     */
    ROLLED_BACK,

    /**
     * Configuration rollout timed out before completion.
     */
    TIMED_OUT
}
