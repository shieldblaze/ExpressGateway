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
package com.shieldblaze.expressgateway.integration;

import java.net.InetAddress;

public interface DNSRecord {

    /**
     * Fully Qualified Domain Name
     */
    String fqdn();

    /**
     * Target IP Address of this Record
     */
    InetAddress target();

    /**
     * Type of DNS Record (A or AAAA)
     */
    RecordType type();

    /**
     * TTL of the record
     */
    long ttl();

    /**
     * DNS Service Provider Name
     */
    String providerName();

    enum RecordType {
        A,
        AAAA
    }
}
