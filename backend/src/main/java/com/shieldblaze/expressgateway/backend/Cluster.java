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
package com.shieldblaze.expressgateway.backend;

import com.shieldblaze.expressgateway.healthcheck.Health;

import java.net.InetSocketAddress;
import java.util.List;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.stream.Collectors;

/**
 * {@link Cluster} is just pool of Backends.
 */
public final class Cluster {
    private final List<Backend> backends = new CopyOnWriteArrayList<>();

    /**
     * Name of Cluster
     */
    private String clusterName;

    /**
     * Get Name of Cluster
     */
    public String getClusterName() {
        return clusterName;
    }

    /**
     * Set Name of Cluster
     */
    public void setClusterName(String clusterName) {
        this.clusterName = clusterName;
    }

    /**
     * Add {@link Backend} into this {@link Cluster}
     */
    public void addBackend(Backend backend) {
        backends.add(backend);
    }

    /**
     * Remote {@link Backend} from this {@link Cluster}
     *
     * @param socketAddress {@link InetSocketAddress} of {@link Backend}
     * @return {@code true} if {@link Backend} is successfully removed else {@code false}
     */
    public boolean removeBackend(InetSocketAddress socketAddress) {
        return backends.removeIf(backend -> backend.getSocketAddress().equals(socketAddress));
    }

    /**
     * Remote {@link Backend} from this {@link Cluster}
     *
     * @return {@code true} if {@link Backend} is successfully removed else {@code false}
     */
    public boolean removeBackend(Backend backend) {
        return backends.remove(backend);
    }

    public List<Backend> getBackends() {
        return backends;
    }

    /**
     * Get {@linkplain List} of available {@linkplain Backend}
     */
    public List<Backend> getAvailableBackends() {
        return backends.stream()
                .filter(backend -> backend.getState() == State.ONLINE)
                .collect(Collectors.toList());
    }
}
