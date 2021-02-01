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

public class CoreConfiguration {

    private TransportConfiguration transportConfiguration;
    private EventLoopConfiguration eventLoopConfiguration;
    private BufferConfiguration bufferConfiguration;
    private EventStreamConfiguration eventStreamConfiguration;
    private HealthCheckConfiguration healthCheckConfiguration;

    public static final CoreConfiguration DEFAULT = new CoreConfiguration();

    static {
        DEFAULT.bufferConfiguration = BufferConfiguration.DEFAULT;
        DEFAULT.eventLoopConfiguration = EventLoopConfiguration.DEFAULT;
        DEFAULT.transportConfiguration = TransportConfiguration.DEFAULT;
        DEFAULT.eventStreamConfiguration = EventStreamConfiguration.DEFAULT;
        DEFAULT.healthCheckConfiguration = HealthCheckConfiguration.DEFAULT;
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
