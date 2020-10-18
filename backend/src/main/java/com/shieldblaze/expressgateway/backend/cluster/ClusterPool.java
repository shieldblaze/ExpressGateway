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

import java.util.Objects;

/**
 * {@linkplain ClusterPool} with multiple {@linkplain Backend}
 */
public final class ClusterPool extends Cluster {

    private static int count = 0;

    public ClusterPool() {
        setName("ClusterPool#" + count++);
    }

    private ClusterPool(String name, Backend... backends) {
        setName(name);
        addBackends(backends);
    }

    public static ClusterPool of(Backend... backends) {
        return new ClusterPool("ClusterPool#" + count++, backends);
    }

    public static ClusterPool of(String name, Backend... backends) {
        return new ClusterPool(name, backends);
    }

    /**
     * @see Cluster#addBackend(Backend)
     */
    public void addBackends(Backend... backends) {
        for (Backend backend : Objects.requireNonNull(backends, "allBackends")) {
            addBackend(backend);
        }
    }
}
