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

public final class CommonConfigurationBuilder {
    private TransportConfiguration transportConfiguration;
    private EventLoopConfiguration eventLoopConfiguration;
    private PooledByteBufAllocatorConfiguration pooledByteBufAllocatorConfiguration;

    private CommonConfigurationBuilder() {
    }

    public static CommonConfigurationBuilder newBuilder() {
        return new CommonConfigurationBuilder();
    }

    public CommonConfigurationBuilder withTransportConfiguration(TransportConfiguration transportConfiguration) {
        this.transportConfiguration = transportConfiguration;
        return this;
    }

    public CommonConfigurationBuilder withEventLoopConfiguration(EventLoopConfiguration eventLoopConfiguration) {
        this.eventLoopConfiguration = eventLoopConfiguration;
        return this;
    }

    public CommonConfigurationBuilder withPooledByteBufAllocatorConfiguration(PooledByteBufAllocatorConfiguration pooledByteBufAllocatorConfiguration) {
        this.pooledByteBufAllocatorConfiguration = pooledByteBufAllocatorConfiguration;
        return this;
    }

    public CommonConfiguration build() {
        CommonConfiguration commonConfiguration = new CommonConfiguration();
        commonConfiguration.setTransportConfiguration(ObjectUtil.checkNotNull(transportConfiguration, "Transport Configuration"));
        commonConfiguration.setEventLoopConfiguration(ObjectUtil.checkNotNull(eventLoopConfiguration, "EventLoop Configuration"));
        commonConfiguration.setPooledByteBufAllocatorConfiguration(ObjectUtil.checkNotNull(pooledByteBufAllocatorConfiguration,
                "PooledByteBufAllocator Configuration"));
        return commonConfiguration;
    }
}
