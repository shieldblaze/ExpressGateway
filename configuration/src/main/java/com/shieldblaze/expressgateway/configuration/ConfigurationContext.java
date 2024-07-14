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
import com.shieldblaze.expressgateway.configuration.http.HttpConfiguration;
import com.shieldblaze.expressgateway.configuration.tls.TlsClientConfiguration;
import com.shieldblaze.expressgateway.configuration.tls.TlsServerConfiguration;
import com.shieldblaze.expressgateway.configuration.transport.TransportConfiguration;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

public record ConfigurationContext(BufferConfiguration bufferConfiguration,
                                   EventLoopConfiguration eventLoopConfiguration,
                                   EventStreamConfiguration eventStreamConfiguration,
                                   HealthCheckConfiguration healthCheckConfiguration,
                                   HttpConfiguration httpConfiguration,
                                   TlsClientConfiguration tlsClientConfiguration,
                                   TlsServerConfiguration tlsServerConfiguration,
                                   TransportConfiguration transportConfiguration) {

    private static final Logger logger = LogManager.getLogger(ConfigurationContext.class);

    /**
     * Default instance of {@link ConfigurationContext} with default configurations
     */
    public static final ConfigurationContext DEFAULT = new ConfigurationContext(
            BufferConfiguration.DEFAULT,
            EventLoopConfiguration.DEFAULT,
            EventStreamConfiguration.DEFAULT,
            HealthCheckConfiguration.DEFAULT,
            HttpConfiguration.DEFAULT,
            TlsClientConfiguration.DEFAULT,
            TlsServerConfiguration.DEFAULT,
            TransportConfiguration.DEFAULT
    );

    public static ConfigurationContext create(Configuration<?>... configurations) {
        BufferConfiguration bufferConfiguration = BufferConfiguration.DEFAULT;
        EventLoopConfiguration eventLoopConfiguration = EventLoopConfiguration.DEFAULT;
        EventStreamConfiguration eventStreamConfiguration = EventStreamConfiguration.DEFAULT;
        HealthCheckConfiguration healthCheckConfiguration = HealthCheckConfiguration.DEFAULT;
        HttpConfiguration httpConfiguration = HttpConfiguration.DEFAULT;
        TlsClientConfiguration tlsClientConfiguration = TlsClientConfiguration.DEFAULT;
        TlsServerConfiguration tlsServerConfiguration = TlsServerConfiguration.DEFAULT;
        TransportConfiguration transportConfiguration = TransportConfiguration.DEFAULT;

        for (Configuration<?> configuration : configurations) {
            if (configuration instanceof BufferConfiguration) {
                bufferConfiguration = (BufferConfiguration) configuration;
            } else if (configuration instanceof EventLoopConfiguration) {
                eventLoopConfiguration = (EventLoopConfiguration) configuration;
            } else if (configuration instanceof EventStreamConfiguration) {
                eventStreamConfiguration = (EventStreamConfiguration) configuration;
            } else if (configuration instanceof HealthCheckConfiguration) {
                healthCheckConfiguration = (HealthCheckConfiguration) configuration;
            } else if (configuration instanceof HttpConfiguration) {
                httpConfiguration = (HttpConfiguration) configuration;
            } else if (configuration instanceof TlsClientConfiguration) {
                tlsClientConfiguration = (TlsClientConfiguration) configuration;
            } else if (configuration instanceof TlsServerConfiguration) {
                tlsServerConfiguration = (TlsServerConfiguration) configuration;
            } else if (configuration instanceof TransportConfiguration) {
                transportConfiguration = (TransportConfiguration) configuration;
            } else {
                throw new IllegalArgumentException("Unknown Configuration: " + configuration);
            }
        }

        return new ConfigurationContext(
                bufferConfiguration,
                eventLoopConfiguration,
                eventStreamConfiguration,
                healthCheckConfiguration,
                httpConfiguration,
                tlsClientConfiguration,
                tlsServerConfiguration,
                transportConfiguration);
    }

}
