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
package com.shieldblaze.expressgateway.backend.cluster;

import com.shieldblaze.expressgateway.backend.Backend;

import java.util.concurrent.atomic.AtomicInteger;

/**
 * {@linkplain Cluster} with single {@linkplain Backend}
 */
public final class SingleBackendCluster extends Cluster {

    private static final AtomicInteger count = new AtomicInteger();

    private SingleBackendCluster(String name, String hostname, Backend backend) {
        name(name);
        hostname(hostname);
        addBackend(backend);
    }

    public static SingleBackendCluster of(Backend backend) {
        return new SingleBackendCluster("SingleBackendCluster#" + count.getAndIncrement(), backend.socketAddress().getHostName(), backend);
    }

    public static SingleBackendCluster of(String hostname, Backend backend) {
        return new SingleBackendCluster("SingleBackendCluster#" + count.getAndIncrement(), hostname, backend);
    }

    public static SingleBackendCluster of(String name, String hostname, Backend backend) {
        return new SingleBackendCluster(name, hostname, backend);
    }
}
