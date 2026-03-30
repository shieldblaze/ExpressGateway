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
package com.shieldblaze.expressgateway.configuration.transport;

/**
 * HAProxy PROXY protocol mode configuration.
 *
 * <ul>
 *   <li>{@link #OFF} - PROXY protocol disabled (default)</li>
 *   <li>{@link #V1} - Accept only PROXY protocol v1 (text-based)</li>
 *   <li>{@link #V2} - Accept only PROXY protocol v2 (binary)</li>
 *   <li>{@link #AUTO} - Auto-detect v1 or v2 based on the initial bytes</li>
 * </ul>
 */
public enum ProxyProtocolMode {

    /**
     * PROXY protocol disabled
     */
    OFF,

    /**
     * PROXY protocol v1 only (text-based: {@code PROXY TCP4 srcIP dstIP srcPort dstPort\r\n})
     */
    V1,

    /**
     * PROXY protocol v2 only (binary header with signature)
     */
    V2,

    /**
     * Auto-detect: inspect the first bytes to determine v1 or v2
     */
    AUTO
}
