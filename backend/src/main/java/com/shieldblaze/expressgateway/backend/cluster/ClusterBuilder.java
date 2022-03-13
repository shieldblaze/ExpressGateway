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
package com.shieldblaze.expressgateway.backend.cluster;

import com.shieldblaze.expressgateway.backend.healthcheck.HealthCheckTemplate;
import com.shieldblaze.expressgateway.backend.loadbalance.LoadBalance;
import com.shieldblaze.expressgateway.configuration.healthcheck.HealthCheckConfiguration;

import java.util.Objects;

/**
 * Builder for {@link Cluster}
 */
public final class ClusterBuilder {

    private LoadBalance<?, ?, ?, ?> loadBalance;
    private HealthCheckConfiguration healthCheckConfiguration;
    private HealthCheckTemplate healthCheckTemplate;

    public static ClusterBuilder newBuilder() {
        return new ClusterBuilder();
    }

    public ClusterBuilder withLoadBalance(LoadBalance<?, ?, ?, ?> loadBalance) {
        this.loadBalance = loadBalance;
        return this;
    }

    public ClusterBuilder withHealthCheck(HealthCheckConfiguration healthCheckConfiguration,
                                          HealthCheckTemplate healthCheckTemplate) {
        this.healthCheckConfiguration = Objects.requireNonNull(healthCheckConfiguration,
                "HealthCheckConfiguration cannot be 'null'");
        this.healthCheckTemplate = Objects.requireNonNull(healthCheckTemplate,
                "HealthCheckTemplate cannot be 'null'");
        return this;
    }

    public Cluster build() {
        Objects.requireNonNull(loadBalance, "LoadBalance cannot be 'null'");
        Cluster cluster = new Cluster(loadBalance);

        // If HealthCheck configuration is available then apply it.
        if (healthCheckConfiguration != null) {
            cluster.configureHealthCheck(healthCheckConfiguration, healthCheckTemplate);
        }

        return cluster;
    }

    private ClusterBuilder() {
        // Prevent outside initialization
    }
}
