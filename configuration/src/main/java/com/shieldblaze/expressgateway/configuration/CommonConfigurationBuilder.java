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
package com.shieldblaze.expressgateway.configuration;

import com.shieldblaze.expressgateway.configuration.buffer.PooledByteBufAllocatorConfiguration;
import com.shieldblaze.expressgateway.configuration.eventloop.EventLoopConfiguration;
import com.shieldblaze.expressgateway.configuration.transport.TransportConfiguration;
import io.netty.util.internal.ObjectUtil;

/**
 * Configuration Builder for {@link CommonConfiguration}
 */
public final class CommonConfigurationBuilder {
    private TransportConfiguration transportConfiguration;
    private EventLoopConfiguration eventLoopConfiguration;
    private PooledByteBufAllocatorConfiguration pooledByteBufAllocatorConfiguration;

    private CommonConfigurationBuilder() {
        // Prevent outside initialization
    }

    /**
     * Create a new {@link CommonConfiguration} Instance
     *
     * @return {@link CommonConfiguration} Instance
     */
    public static CommonConfigurationBuilder newBuilder() {
        return new CommonConfigurationBuilder();
    }

    /**
     * Set {@link TransportConfiguration}
     */
    public CommonConfigurationBuilder withTransportConfiguration(TransportConfiguration transportConfiguration) {
        this.transportConfiguration = transportConfiguration;
        return this;
    }

    /**
     * Set {@link EventLoopConfiguration}
     */
    public CommonConfigurationBuilder withEventLoopConfiguration(EventLoopConfiguration eventLoopConfiguration) {
        this.eventLoopConfiguration = eventLoopConfiguration;
        return this;
    }

    /**
     * Set {@link PooledByteBufAllocatorConfiguration}
     */
    public CommonConfigurationBuilder withPooledByteBufAllocatorConfiguration(PooledByteBufAllocatorConfiguration pooledByteBufAllocatorConfiguration) {
        this.pooledByteBufAllocatorConfiguration = pooledByteBufAllocatorConfiguration;
        return this;
    }

    /**
     * Build {@link CommonConfiguration}
     *
     * @return {@link CommonConfiguration} Instance
     * @throws NullPointerException If a required value if {@code null}
     */
    public CommonConfiguration build() {
        return new CommonConfiguration()
                .transportConfiguration(ObjectUtil.checkNotNull(transportConfiguration, "Transport Configuration"))
                .eventLoopConfiguration(ObjectUtil.checkNotNull(eventLoopConfiguration, "EventLoop Configuration"))
                .pooledByteBufAllocatorConfiguration(ObjectUtil.checkNotNull(pooledByteBufAllocatorConfiguration, "PooledByteBufAllocator Configuration"));
    }
}
