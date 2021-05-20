/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2021 ShieldBlaze
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

import com.shieldblaze.expressgateway.backend.healthcheck.HealthCheckTemplate;
import com.shieldblaze.expressgateway.backend.loadbalance.LoadBalance;
import com.shieldblaze.expressgateway.configuration.healthcheck.HealthCheckConfiguration;

import java.util.Objects;

/**
 * Builder for {@link Cluster}
 */
public final class ClusterBuilder {

    private ClusterBuilder() {
        // Prevent outside initialization
    }

    public static ClusterBuilder newBuilder() {
        return new ClusterBuilder();
    }

    private LoadBalance<?, ?, ?, ?> loadBalance;
    private HealthCheckConfiguration healthCheckConfiguration;
    private HealthCheckTemplate healthCheckTemplate;

    public ClusterBuilder withLoadBalance(LoadBalance<?, ?, ?, ?> loadBalance) {
        this.loadBalance = loadBalance;
        return this;
    }

    public ClusterBuilder withHealthCheck(HealthCheckConfiguration healthCheckConfiguration, HealthCheckTemplate healthCheckTemplate) {
        this.healthCheckConfiguration = Objects.requireNonNull(healthCheckConfiguration, "HealthCheckConfiguration");
        this.healthCheckTemplate = Objects.requireNonNull(healthCheckTemplate, "HealthCheckTemplate");
        return this;
    }

    public Cluster build() {
        Objects.requireNonNull(loadBalance, "LoadBalance");
        Cluster cluster = new Cluster(loadBalance);

        if (healthCheckConfiguration != null) {
            cluster.configureHealthCheck(healthCheckConfiguration, healthCheckTemplate);
        }

        return cluster;
    }
}
