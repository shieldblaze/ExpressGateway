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

import com.shieldblaze.expressgateway.core.configuration.buffer.PooledByteBufAllocatorConfiguration;
import com.shieldblaze.expressgateway.core.configuration.eventloop.EventLoopConfiguration;
import com.shieldblaze.expressgateway.core.configuration.transport.TransportConfiguration;
import io.netty.util.internal.ObjectUtil;

public final class ConfigurationBuilder {
    private TransportConfiguration transportConfiguration;
    private EventLoopConfiguration eventLoopConfiguration;
    private PooledByteBufAllocatorConfiguration pooledByteBufAllocatorConfiguration;

    private ConfigurationBuilder() {
    }

    public static ConfigurationBuilder newBuilder() {
        return new ConfigurationBuilder();
    }

    public ConfigurationBuilder withTransportConfiguration(TransportConfiguration transportConfiguration) {
        this.transportConfiguration = transportConfiguration;
        return this;
    }

    public ConfigurationBuilder withEventLoopConfiguration(EventLoopConfiguration eventLoopConfiguration) {
        this.eventLoopConfiguration = eventLoopConfiguration;
        return this;
    }

    public ConfigurationBuilder withPooledByteBufAllocatorConfiguration(PooledByteBufAllocatorConfiguration pooledByteBufAllocatorConfiguration) {
        this.pooledByteBufAllocatorConfiguration = pooledByteBufAllocatorConfiguration;
        return this;
    }

    public Configuration build() {
        Configuration configuration = new Configuration();
        configuration.setTransportConfiguration(ObjectUtil.checkNotNull(transportConfiguration, "Transport Configuration"));
        configuration.setEventLoopConfiguration(ObjectUtil.checkNotNull(eventLoopConfiguration, "EventLoop Configuration"));
        configuration.setPooledByteBufAllocatorConfiguration(ObjectUtil.checkNotNull(pooledByteBufAllocatorConfiguration,
                "PooledByteBufAllocator Configuration"));
        return configuration;
    }
}
