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
package com.shieldblaze.expressgateway.protocol.tcp.loadbalancer;

import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.strategy.l4.L4Balance;
import com.shieldblaze.expressgateway.configuration.CoreConfiguration;
import com.shieldblaze.expressgateway.core.L4FrontListener;
import com.shieldblaze.expressgateway.core.L4LoadBalancer;
import io.netty.channel.ChannelHandler;

import java.net.InetSocketAddress;

/**
 * Default implementation for {@link L4LoadBalancer}
 */
public final class DefaultL4LoadBalancer extends L4LoadBalancer {

    /**
     * @param bindAddress       {@link InetSocketAddress} on which {@link L4FrontListener} will bind and listen.
     * @param l4Balance         {@link L4Balance} for Load Balance
     * @param l4FrontListener   {@link L4FrontListener} for listening and handling traffic
     * @param cluster           {@link Cluster} to be Load Balanced
     * @param coreConfiguration {@link CoreConfiguration} to be applied
     * @throws NullPointerException If a required parameter if {@code null}
     */
    public DefaultL4LoadBalancer(InetSocketAddress bindAddress, L4Balance l4Balance, L4FrontListener l4FrontListener, Cluster cluster,
                                 CoreConfiguration coreConfiguration) {
        this(bindAddress, l4Balance, l4FrontListener, cluster, coreConfiguration, null);
    }

    /**
     * @param bindAddress       {@link InetSocketAddress} on which {@link L4FrontListener} will bind and listen.
     * @param l4Balance         {@link L4Balance} for Load Balance
     * @param l4FrontListener   {@link L4FrontListener} for listening and handling traffic
     * @param cluster           {@link Cluster} to be Load Balanced
     * @param coreConfiguration {@link CoreConfiguration} to be applied
     * @param channelHandler    {@link ChannelHandler} to use for handling traffic
     * @throws NullPointerException If a required parameter if {@code null}
     */
    public DefaultL4LoadBalancer(InetSocketAddress bindAddress, L4Balance l4Balance, L4FrontListener l4FrontListener, Cluster cluster,
                                 CoreConfiguration coreConfiguration, ChannelHandler channelHandler) {
        super(bindAddress, l4Balance, l4FrontListener, cluster, coreConfiguration, channelHandler);
    }
}
