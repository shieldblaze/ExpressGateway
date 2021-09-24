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
package com.shieldblaze.expressgateway.restapi.api.cluster;

import com.fasterxml.jackson.annotation.JsonProperty;
import com.shieldblaze.expressgateway.backend.healthcheck.HealthCheckTemplate;

import java.util.Objects;

public final class ClusterContext {

    @JsonProperty("hostname")
    private String hostname;

    @JsonProperty("loadBalance")
    private String loadBalance;

    @JsonProperty("sessionPersistence")
    private String sessionPersistence;

    @JsonProperty("healthCheckTemplate")
    private HealthCheckTemplate healthCheckTemplate;

    public void setLoadBalance(String loadBalance) {
        this.loadBalance = Objects.requireNonNull(loadBalance, "LoadBalance");
    }

    public void setSessionPersistence(String sessionPersistence) {
        this.sessionPersistence = Objects.requireNonNull(sessionPersistence, "SessionPersistence");
    }

    public String hostname() {
        return hostname;
    }

    public String loadBalance() {
        return loadBalance;
    }

    public String sessionPersistence() {
        return sessionPersistence;
    }

    public HealthCheckTemplate healthCheckTemplate() {
        return healthCheckTemplate;
    }
}
