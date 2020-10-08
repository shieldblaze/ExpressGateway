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
package com.shieldblaze.expressgateway.core.configuration;

import com.shieldblaze.expressgateway.core.l4.L4LoadBalancer;
import com.shieldblaze.expressgateway.core.configuration.buffer.PooledByteBufAllocatorConfiguration;
import com.shieldblaze.expressgateway.core.configuration.eventloop.EventLoopConfiguration;
import com.shieldblaze.expressgateway.core.configuration.transport.TransportConfiguration;

/**
 * Common Configuration can be shared between multiple {@link L4LoadBalancer} because it contains
 * configuration that can easily be re-used.
 */
public class CommonConfiguration extends Configuration {

    private TransportConfiguration transportConfiguration;
    private EventLoopConfiguration eventLoopConfiguration;
    private PooledByteBufAllocatorConfiguration pooledByteBufAllocatorConfiguration;

    public TransportConfiguration getTransportConfiguration() {
        return transportConfiguration;
    }

    void setTransportConfiguration(TransportConfiguration transportConfiguration) {
        this.transportConfiguration = transportConfiguration;
    }

    public EventLoopConfiguration getEventLoopConfiguration() {
        return eventLoopConfiguration;
    }

    void setEventLoopConfiguration(EventLoopConfiguration eventLoopConfiguration) {
        this.eventLoopConfiguration = eventLoopConfiguration;
    }

    public PooledByteBufAllocatorConfiguration getPooledByteBufAllocatorConfiguration() {
        return pooledByteBufAllocatorConfiguration;
    }

    void setPooledByteBufAllocatorConfiguration(PooledByteBufAllocatorConfiguration pooledByteBufAllocatorConfiguration) {
        this.pooledByteBufAllocatorConfiguration = pooledByteBufAllocatorConfiguration;
    }
}
