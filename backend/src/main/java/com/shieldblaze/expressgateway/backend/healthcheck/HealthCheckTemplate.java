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
package com.shieldblaze.expressgateway.backend.healthcheck;

import com.fasterxml.jackson.annotation.JsonProperty;
import com.shieldblaze.expressgateway.common.JakartaValidator;
import jakarta.validation.constraints.Max;
import jakarta.validation.constraints.Min;
import jakarta.validation.constraints.NotNull;
import lombok.Getter;
import lombok.ToString;
import lombok.experimental.Accessors;

@Getter
@ToString
@Accessors(fluent = true)
public final class HealthCheckTemplate {

    /**
     * Health Check Protocol
     */
    @JsonProperty("protocol")
    @NotNull
    private Protocol protocol;

    /**
     * Health Check Host
     */
    @NotNull
    @JsonProperty("host")
    private String host;

    /**
     * Health Check Port
     */
    @Min(1)
    @Max(65535)
    @JsonProperty("port")
    private int port;

    /**
     * HTTP Path
     */
    @NotNull
    @JsonProperty("path")
    private String path;

    /**
     * Timeout in seconds
     */
    @JsonProperty("timeout")
    private int timeout;

    /**
     * Number of Health Check samples
     */
    @JsonProperty("samples")
    private int samples;

    HealthCheckTemplate() {
        // For builder
    }

    public static Builder builder() {
        return new Builder();
    }

    public static class Builder {

        private Protocol protocol;
        private String host;
        private int port;
        private String path;
        private int timeout;
        private int samples;

        public Builder protocol(Protocol protocol) {
            this.protocol = protocol;
            return this;
        }

        public Builder host(String host) {
            this.host = host;
            return this;
        }

        public Builder port(int port) {
            this.port = port;
            return this;
        }

        public Builder path(String path) {
            this.path = path;
            return this;
        }

        public Builder timeout(int timeout) {
            this.timeout = timeout;
            return this;
        }

        public Builder samples(int samples) {
            this.samples = samples;
            return this;
        }

        public HealthCheckTemplate build() {
            HealthCheckTemplate instance = new HealthCheckTemplate();
            instance.protocol = this.protocol;
            instance.host = this.host;
            instance.port = this.port;
            instance.path = this.path;
            instance.timeout = this.timeout;
            instance.samples = this.samples;
            JakartaValidator.validate(instance);
            return instance;
        }
    }

    public enum Protocol {
        TCP,
        UDP,
        HTTP,
        HTTPS
    }
}
