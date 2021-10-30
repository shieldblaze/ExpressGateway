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
package com.shieldblaze.expressgateway.configuration;

import com.shieldblaze.expressgateway.configuration.buffer.BufferConfiguration;
import com.shieldblaze.expressgateway.configuration.eventloop.EventLoopConfiguration;
import com.shieldblaze.expressgateway.configuration.eventstream.EventStreamConfiguration;
import com.shieldblaze.expressgateway.configuration.healthcheck.HealthCheckConfiguration;
import com.shieldblaze.expressgateway.configuration.transport.TransportConfiguration;

/**
 * This class holds {@link TransportConfiguration}, {@link EventStreamConfiguration},
 * {@link BufferConfiguration}, {@link EventStreamConfiguration} and
 * {@link HealthCheckConfiguration} because they're core and vital part of ExpressGateway.
 */
public final class CoreConfiguration {

    private TransportConfiguration transportConfiguration;
    private EventLoopConfiguration eventLoopConfiguration;
    private BufferConfiguration bufferConfiguration;
    private EventStreamConfiguration eventStreamConfiguration;
    private HealthCheckConfiguration healthCheckConfiguration;

    /**
     * Default instance of {@link CoreConfiguration}
     */
    public static final CoreConfiguration INSTANCE = new CoreConfiguration();

    static {
        INSTANCE.bufferConfiguration = BufferConfiguration.load();
        INSTANCE.eventLoopConfiguration = EventLoopConfiguration.load();
        INSTANCE.transportConfiguration = TransportConfiguration.load();
        INSTANCE.eventStreamConfiguration = EventStreamConfiguration.load();
        INSTANCE.healthCheckConfiguration = HealthCheckConfiguration.load();
    }

    public TransportConfiguration transportConfiguration() {
        return transportConfiguration;
    }

    CoreConfiguration transportConfiguration(TransportConfiguration transportConfiguration) {
        this.transportConfiguration = transportConfiguration;
        return this;
    }

    public EventLoopConfiguration eventLoopConfiguration() {
        return eventLoopConfiguration;
    }

    CoreConfiguration eventLoopConfiguration(EventLoopConfiguration eventLoopConfiguration) {
        this.eventLoopConfiguration = eventLoopConfiguration;
        return this;
    }

    public BufferConfiguration bufferConfiguration() {
        return bufferConfiguration;
    }

    CoreConfiguration bufferConfiguration(BufferConfiguration bufferConfiguration) {
        this.bufferConfiguration = bufferConfiguration;
        return this;
    }

    public EventStreamConfiguration eventStreamConfiguration() {
        return eventStreamConfiguration;
    }

    CoreConfiguration eventStreamConfiguration(EventStreamConfiguration eventStreamConfiguration) {
        this.eventStreamConfiguration = eventStreamConfiguration;
        return this;
    }

    public HealthCheckConfiguration healthCheckConfiguration() {
        return healthCheckConfiguration;
    }

    CoreConfiguration healthCheckConfiguration(HealthCheckConfiguration healthCheckConfiguration) {
        this.healthCheckConfiguration = healthCheckConfiguration;
        return this;
    }
}
