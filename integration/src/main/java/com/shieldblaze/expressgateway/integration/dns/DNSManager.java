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
package com.shieldblaze.expressgateway.integration.dns;

import com.shieldblaze.expressgateway.integration.event.DNSAddedEvent;
import com.shieldblaze.expressgateway.integration.event.DNSRemovedEvent;

import java.util.List;

/**
 * This class is used for adding or removing records.
 *
 * @param <T> Record type
 */
public interface DNSManager<T> {

    /**
     * List of DNS Record
     */
    List<DNSRecord> dnsRecords();

    /**
     * Add a new DNS Record
     */
    DNSAddedEvent<?> add(T add);

    /**
     * Remove a existing DNS Record
     */
    DNSRemovedEvent<?> remove(T remove);
}
