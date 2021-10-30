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

import com.shieldblaze.expressgateway.concurrent.eventstream.EventStream;
import com.shieldblaze.expressgateway.configuration.autoscaling.AutoscalingConfiguration;
import com.shieldblaze.expressgateway.configuration.buffer.BufferConfiguration;
import com.shieldblaze.expressgateway.configuration.eventloop.EventLoopConfiguration;
import com.shieldblaze.expressgateway.configuration.eventstream.EventStreamConfiguration;
import com.shieldblaze.expressgateway.configuration.exceptions.ProfileNotFoundException;
import com.shieldblaze.expressgateway.configuration.healthcheck.HealthCheckConfiguration;
import com.shieldblaze.expressgateway.configuration.http.HTTPConfiguration;
import com.shieldblaze.expressgateway.configuration.tls.TLSConfiguration;
import com.shieldblaze.expressgateway.configuration.transport.TransportConfiguration;

import java.io.IOException;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

/**
 * {@link Profile} handles {@link Configuration} instances.
 */
public class Profile {

    /**
     * Name of this profile
     */
    private String name;

    /**
     * This Map holds {@link Configuration} implementation instances
     * which will be queried upon request.
     *
     * Elements:
     * 1. Autoscaling
     * 2. Buffer
     * 3. EventLoop
     * 4. EventStream
     * 5. HealthCheck
     * 6. HTTP
     * 7. TLS
     * 8. Transport
     */
    private final Map<String, Configuration<?>> configurationMap = new HashMap<>(8);

    public static void main(String[] args) throws IOException {
        Profile profile = new Profile();
        profile.name = "GG";
        System.out.println(Store.load(profile, BufferConfiguration.class));
    }

    public static Profile load(String name) throws ProfileNotFoundException {
        try {
            Profile profile = new Profile();

            AutoscalingConfiguration autoscaling = Store.load(profile, AutoscalingConfiguration.class);
            BufferConfiguration buffer = Store.load(profile, BufferConfiguration.class);
            EventLoopConfiguration eventLoop = Store.load(profile, EventLoopConfiguration.class);
            EventStreamConfiguration eventStream = Store.load(profile, EventStreamConfiguration.class);
            HealthCheckConfiguration healthCheck = Store.load(profile, HealthCheckConfiguration.class);
            HTTPConfiguration http = Store.load(profile, HTTPConfiguration.class);
            TLSConfiguration tls = Store.load(profile, TLSConfiguration.class);
            TransportConfiguration transport = Store.load(profile, TransportConfiguration.class);

            return null;
        } catch (Exception ex) {
            throw new ProfileNotFoundException("Profile not found: " + name);
        }
    }

    public String name() {
        return name;
    }

    private Profile() {
        // Prevent outside initialization
    }
}
