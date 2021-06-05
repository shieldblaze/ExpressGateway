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
package com.shieldblaze.expressgateway.configuration;

import com.shieldblaze.expressgateway.configuration.buffer.BufferConfiguration;
import com.shieldblaze.expressgateway.configuration.eventloop.EventLoopConfiguration;
import com.shieldblaze.expressgateway.configuration.eventstream.EventStreamConfiguration;
import com.shieldblaze.expressgateway.configuration.healthcheck.HealthCheckConfiguration;
import com.shieldblaze.expressgateway.configuration.transport.TransportConfiguration;

import java.util.Objects;

/**
 * Configuration Builder for {@link CoreConfiguration}
 */
public final class CoreConfigurationBuilder {
    private TransportConfiguration transportConfiguration;
    private EventLoopConfiguration eventLoopConfiguration;
    private BufferConfiguration bufferConfiguration;
    private EventStreamConfiguration eventStreamConfiguration;
    private HealthCheckConfiguration healthCheckConfiguration;

    private CoreConfigurationBuilder() {
        // Prevent outside initialization
    }

    /**
     * Create a new {@link CoreConfiguration} Instance
     *
     * @return {@link CoreConfiguration} Instance
     */
    public static CoreConfigurationBuilder newBuilder() {
        return new CoreConfigurationBuilder();
    }

    /**
     * Set {@link TransportConfiguration}
     */
    public CoreConfigurationBuilder withTransportConfiguration(TransportConfiguration transportConfiguration) {
        this.transportConfiguration = transportConfiguration;
        return this;
    }

    /**
     * Set {@link EventLoopConfiguration}
     */
    public CoreConfigurationBuilder withEventLoopConfiguration(EventLoopConfiguration eventLoopConfiguration) {
        this.eventLoopConfiguration = eventLoopConfiguration;
        return this;
    }

    /**
     * Set {@link BufferConfiguration}
     */
    public CoreConfigurationBuilder withBufferConfiguration(BufferConfiguration bufferConfiguration) {
        this.bufferConfiguration = bufferConfiguration;
        return this;
    }

    /**
     * Set {@link EventStreamConfiguration}
     */
    public CoreConfigurationBuilder withEventStreamConfiguration(EventStreamConfiguration eventStreamConfiguration) {
        this.eventStreamConfiguration = eventStreamConfiguration;
        return this;
    }

    /**
     * Set {@link HealthCheckConfiguration}
     */
    public CoreConfigurationBuilder withHealthCheckConfiguration(HealthCheckConfiguration healthCheckConfiguration) {
        this.healthCheckConfiguration = healthCheckConfiguration;
        return this;
    }

    /**
     * Build {@link CoreConfiguration}
     *
     * @return {@link CoreConfiguration} Instance
     * @throws NullPointerException If a required value if {@code null}
     */
    public CoreConfiguration build() {
        Objects.requireNonNull(transportConfiguration, "Transport Configuration");
        Objects.requireNonNull(eventLoopConfiguration, "EventLoop Configuration");
        Objects.requireNonNull(bufferConfiguration, "Buffer Configuration");
        Objects.requireNonNull(eventStreamConfiguration, "EventStream Configuration");
        Objects.requireNonNull(healthCheckConfiguration, "HealthCheck Configuration");

        return new CoreConfiguration()
                .transportConfiguration(transportConfiguration)
                .eventLoopConfiguration(eventLoopConfiguration)
                .bufferConfiguration(bufferConfiguration)
                .eventStreamConfiguration(eventStreamConfiguration)
                .healthCheckConfiguration(healthCheckConfiguration);
    }
}
