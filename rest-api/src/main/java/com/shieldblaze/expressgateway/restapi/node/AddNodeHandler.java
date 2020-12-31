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
package com.shieldblaze.expressgateway.restapi.node;

import com.fasterxml.jackson.annotation.JsonProperty;

final class AddNodeHandler {

    @JsonProperty("host")
    private String host;

    @JsonProperty("port")
    private int port;

    @JsonProperty("maxConnections")
    private int maxConnections;

    @JsonProperty("healthCheck")
    private HealthCheckContext healthCheckContext;

    public String host() {
        return host;
    }

    public int port() {
        return port;
    }

    public int maxConnections() {
        return maxConnections;
    }

    public HealthCheckContext healthCheckContext() {
        return healthCheckContext;
    }

    static final class HealthCheckContext {

        @JsonProperty("host")
        private String host;

        @JsonProperty("port")
        private int port;

        @JsonProperty("timeout")
        private int timeout;

        @JsonProperty("type")
        private String type;

        @JsonProperty("httpPath")
        private String httpPath;

        @JsonProperty("enableTLSValidation")
        private boolean enableTLSValidation;

        public String host() {
            return host;
        }

        public int port() {
            return port;
        }

        public int timeout() {
            return timeout;
        }

        public String type() {
            return type;
        }

        public String httpPath() {
            return httpPath;
        }

        public boolean enableTLSValidation() {
            return enableTLSValidation;
        }
    }
}
