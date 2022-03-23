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

import com.shieldblaze.expressgateway.configuration.autoscaling.AutoscalingConfiguration;
import com.shieldblaze.expressgateway.configuration.buffer.BufferConfiguration;
import com.shieldblaze.expressgateway.configuration.eventloop.EventLoopConfiguration;
import com.shieldblaze.expressgateway.configuration.eventstream.EventStreamConfiguration;
import com.shieldblaze.expressgateway.configuration.healthcheck.HealthCheckConfiguration;
import com.shieldblaze.expressgateway.configuration.http.HttpConfiguration;
import com.shieldblaze.expressgateway.configuration.tls.TLSClientConfiguration;
import com.shieldblaze.expressgateway.configuration.tls.TLSServerConfiguration;
import com.shieldblaze.expressgateway.configuration.transport.TransportConfiguration;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.io.IOException;

public record ConfigurationContext(String profileName,
                                   AutoscalingConfiguration autoscalingConfiguration,
                                   BufferConfiguration bufferConfiguration,
                                   EventLoopConfiguration eventLoopConfiguration,
                                   EventStreamConfiguration eventStreamConfiguration,
                                   HealthCheckConfiguration healthCheckConfiguration,
                                   HttpConfiguration httpConfiguration,
                                   TLSClientConfiguration tlsClientConfiguration,
                                   TLSServerConfiguration tlsServerConfiguration,
                                   TransportConfiguration transportConfiguration) {

    private static final Logger logger = LogManager.getLogger(ConfigurationContext.class);

    /**
     * Default instance of {@link ConfigurationContext} with default configurations
     */
    public static final ConfigurationContext DEFAULT = new ConfigurationContext(
            null,
            null,
            BufferConfiguration.DEFAULT,
            EventLoopConfiguration.DEFAULT,
            EventStreamConfiguration.DEFAULT,
            HealthCheckConfiguration.DEFAULT,
            HttpConfiguration.DEFAULT,
            TLSClientConfiguration.DEFAULT,
            TLSServerConfiguration.DEFAULT,
            TransportConfiguration.DEFAULT
    );

    public static ConfigurationContext create(String profileName, Configuration... configurations) {
        AutoscalingConfiguration autoscalingConfiguration = null;
        BufferConfiguration bufferConfiguration = BufferConfiguration.DEFAULT;
        EventLoopConfiguration eventLoopConfiguration = EventLoopConfiguration.DEFAULT;
        EventStreamConfiguration eventStreamConfiguration = EventStreamConfiguration.DEFAULT;
        HealthCheckConfiguration healthCheckConfiguration = HealthCheckConfiguration.DEFAULT;
        HttpConfiguration httpConfiguration = HttpConfiguration.DEFAULT;
        TLSClientConfiguration tlsClientConfiguration = TLSClientConfiguration.DEFAULT;
        TLSServerConfiguration tlsServerConfiguration = TLSServerConfiguration.DEFAULT;
        TransportConfiguration transportConfiguration = TransportConfiguration.DEFAULT;

        for (Configuration configuration : configurations) {
            if (configuration instanceof AutoscalingConfiguration) {
                autoscalingConfiguration = (AutoscalingConfiguration) configuration;
            } else if (configuration instanceof BufferConfiguration) {
                bufferConfiguration = (BufferConfiguration) configuration;
            } else if (configuration instanceof EventLoopConfiguration) {
                eventLoopConfiguration = (EventLoopConfiguration) configuration;
            } else if (configuration instanceof EventStreamConfiguration) {
                eventStreamConfiguration = (EventStreamConfiguration) configuration;
            } else if (configuration instanceof HealthCheckConfiguration) {
                healthCheckConfiguration = (HealthCheckConfiguration) configuration;
            } else if (configuration instanceof HttpConfiguration) {
                httpConfiguration = (HttpConfiguration) configuration;
            } else if (configuration instanceof TLSClientConfiguration) {
                tlsClientConfiguration = (TLSClientConfiguration) configuration;
            } else if (configuration instanceof TLSServerConfiguration) {
                tlsServerConfiguration = (TLSServerConfiguration) configuration;
            } else if (configuration instanceof TransportConfiguration) {
                transportConfiguration = (TransportConfiguration) configuration;
            } else {
                throw new IllegalArgumentException("Unknown Configuration: " + configuration);
            }
        }

        return new ConfigurationContext(profileName,
                autoscalingConfiguration,
                bufferConfiguration,
                eventLoopConfiguration,
                eventStreamConfiguration,
                healthCheckConfiguration,
                httpConfiguration,
                tlsClientConfiguration,
                tlsServerConfiguration,
                transportConfiguration);
    }

    public ConfigurationContext(String profileName,
                                AutoscalingConfiguration autoscalingConfiguration,
                                BufferConfiguration bufferConfiguration,
                                EventLoopConfiguration eventLoopConfiguration,
                                EventStreamConfiguration eventStreamConfiguration,
                                HealthCheckConfiguration healthCheckConfiguration,
                                HttpConfiguration httpConfiguration,
                                TLSClientConfiguration tlsClientConfiguration,
                                TLSServerConfiguration tlsServerConfiguration,
                                TransportConfiguration transportConfiguration) {
        this.profileName = profileName;
        this.autoscalingConfiguration = autoscalingConfiguration;
        this.bufferConfiguration = bufferConfiguration;
        this.eventLoopConfiguration = eventLoopConfiguration;
        this.eventStreamConfiguration = eventStreamConfiguration;
        this.healthCheckConfiguration = healthCheckConfiguration;
        this.httpConfiguration = httpConfiguration;
        this.tlsClientConfiguration = tlsClientConfiguration;
        this.tlsServerConfiguration = tlsServerConfiguration;
        this.transportConfiguration = transportConfiguration;
    }

    public static ConfigurationContext load(String profileName) throws IOException {
        try {
            AutoscalingConfiguration autoscalingConfiguration = ConfigurationStore.load(profileName, AutoscalingConfiguration.class);
            BufferConfiguration bufferConfiguration = ConfigurationStore.load(profileName, BufferConfiguration.class);
            EventLoopConfiguration eventLoopConfiguration = ConfigurationStore.load(profileName, EventLoopConfiguration.class);
            EventStreamConfiguration eventStreamConfiguration = ConfigurationStore.load(profileName, EventStreamConfiguration.class);
            HealthCheckConfiguration healthCheckConfiguration = ConfigurationStore.load(profileName, HealthCheckConfiguration.class);
            HttpConfiguration httpConfiguration = ConfigurationStore.load(profileName, HttpConfiguration.class);
            TLSClientConfiguration tlsClientConfiguration = ConfigurationStore.load(profileName, TLSClientConfiguration.class);
            TLSServerConfiguration tlsServerConfiguration = ConfigurationStore.load(profileName, TLSServerConfiguration.class);
            TransportConfiguration transportConfiguration = ConfigurationStore.load(profileName, TransportConfiguration.class);

            return new ConfigurationContext(
                    profileName,
                    autoscalingConfiguration,
                    bufferConfiguration,
                    eventLoopConfiguration,
                    eventStreamConfiguration,
                    healthCheckConfiguration,
                    httpConfiguration,
                    tlsClientConfiguration,
                    tlsServerConfiguration,
                    transportConfiguration
            );
        } catch (IOException e) {
            logger.error("Failed to load Profile: " + profileName, e);
            throw e;
        }
    }

    public void save() throws IOException {
        try {
            if (profileName == null) {
                throw new IllegalArgumentException("Cannot save configurations because Profile Name is not present");
            }

            ConfigurationStore.save(profileName, autoscalingConfiguration);
            ConfigurationStore.save(profileName, bufferConfiguration);
            ConfigurationStore.save(profileName, eventLoopConfiguration);
            ConfigurationStore.save(profileName, eventStreamConfiguration);
            ConfigurationStore.save(profileName, healthCheckConfiguration);
            ConfigurationStore.save(profileName, httpConfiguration);
            ConfigurationStore.save(profileName, tlsClientConfiguration);
            ConfigurationStore.save(profileName, tlsServerConfiguration);
            ConfigurationStore.save(profileName, transportConfiguration);
        } catch (IOException e) {
            logger.error("Failed to save Profile: " + profileName);
            throw e;
        }
    }
}
