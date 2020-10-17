/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020 ShieldBlaze
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
package com.shieldblaze.expressgateway.healthcheck;

/**
 * Health of Remote Host
 */
public enum Health {
    /**
     * <p> Health is good. </p>
     * <p> Remote host passes more than 95% of Health Check Successfully </p>
     */
    GOOD,

    /**
     * Health is not good and not bad (medium).
     * <p> Remote host passes more than 75% of Health Check Successfully </p>
     */
    MEDIUM,

    /**
     * Health is bad.
     * <p> Remote host passes less than 75% of Health Check Successfully </p>
     */
    BAD,

    /**
     * Health is unknown.
     */
    UNKNOWN;
}
