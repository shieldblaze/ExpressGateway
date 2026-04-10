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
package com.shieldblaze.expressgateway.coordination;

import java.util.Map;

/**
 * Factory for creating {@link CoordinationProvider} instances from configuration maps.
 *
 * <p>Each backend (ZooKeeper, etcd, Consul) provides an implementation of this
 * factory. Discovery can be done via {@link java.util.ServiceLoader} or explicit
 * registration.</p>
 *
 * <p>Configuration keys are backend-specific. Implementations MUST document their
 * required and optional configuration keys.</p>
 */
public interface CoordinationProviderFactory {

    /**
     * Creates a new {@link CoordinationProvider} from the given configuration.
     * The returned provider is fully initialized and connected.
     *
     * @param config backend-specific configuration (e.g. connection string, timeouts)
     * @return a ready-to-use provider
     * @throws CoordinationException if the provider cannot be created or connected
     */
    CoordinationProvider create(Map<String, String> config) throws CoordinationException;

    /**
     * Returns the backend type identifier for this factory.
     *
     * @return a lowercase identifier (e.g. "zookeeper", "etcd", "consul")
     */
    String type();
}
