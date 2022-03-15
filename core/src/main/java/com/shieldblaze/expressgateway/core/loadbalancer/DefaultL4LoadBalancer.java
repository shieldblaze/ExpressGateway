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
package com.shieldblaze.expressgateway.core.loadbalancer;

import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.configuration.ConfigurationContext;
import com.shieldblaze.expressgateway.core.L4FrontListener;
import io.netty.channel.ChannelHandler;

import java.net.InetSocketAddress;

/**
 * Default implementation for {@link L4LoadBalancer}
 */
final class DefaultL4LoadBalancer extends L4LoadBalancer {

    /**
     * @param name                 Name of this Load Balancer
     * @param bindAddress          {@link InetSocketAddress} on which {@link L4FrontListener} will bind and listen.
     * @param l4FrontListener      {@link L4FrontListener} for listening traffic
     * @param configurationContext {@link ConfigurationContext} to be applied
     * @param channelHandler       {@link ChannelHandler} to use for handling traffic
     * @throws NullPointerException If a required parameter if {@code null}
     */
    DefaultL4LoadBalancer(String name, InetSocketAddress bindAddress, L4FrontListener l4FrontListener, ConfigurationContext configurationContext,
                          ChannelHandler channelHandler) {
        super(name, bindAddress, l4FrontListener, configurationContext, channelHandler);
    }

    @Override
    public Cluster cluster(String hostname) {
        return super.cluster(DEFAULT);
    }

    @Override
    public void mapCluster(String hostname, Cluster cluster) {
        super.mapCluster(DEFAULT, cluster);
    }

    @Override
    public void remapCluster(String oldHostname, String newHostname) {
        super.remapCluster(DEFAULT, "DEFAULT");
    }

    @Override
    public boolean removeCluster(String hostname) {
        return super.removeCluster(DEFAULT);
    }

    @Override
    public String type() {
        return "L4";
    }
}
