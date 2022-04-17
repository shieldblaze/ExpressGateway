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
package com.shieldblaze.expressgateway.configuration.healthcheck;

import com.fasterxml.jackson.annotation.JsonIgnore;
import com.fasterxml.jackson.annotation.JsonProperty;
import com.shieldblaze.expressgateway.common.utils.NumberUtil;
import com.shieldblaze.expressgateway.configuration.Configuration;
import dev.morphia.annotations.Entity;
import dev.morphia.annotations.Id;
import dev.morphia.annotations.Property;
import dev.morphia.annotations.Transient;

import java.util.Objects;
import java.util.UUID;

/**
 * Configuration for {@link HealthCheckConfiguration}
 */
@Entity(value = "HealthCheck", useDiscriminator = false)
public final class HealthCheckConfiguration implements Configuration<HealthCheckConfiguration> {

    @Id
    @JsonProperty
    private String id;

    @Property
    @JsonProperty
    private String profileName;

    @Property
    @JsonProperty
    private int workers;

    @Property
    @JsonProperty
    private int timeInterval;

    @Transient
    @JsonIgnore
    private boolean validated;

    public static final HealthCheckConfiguration DEFAULT = new HealthCheckConfiguration();

    static {
        DEFAULT.id = "default";
        DEFAULT.profileName = "default";
        DEFAULT.workers = Runtime.getRuntime().availableProcessors();
        DEFAULT.timeInterval = 1;
        DEFAULT.validated = true;
    }

    /**
     * Profile name
     */
    public HealthCheckConfiguration setProfileName(String profileName) {
        this.profileName = profileName;
        return this;
    }

    /**
     * Profile name
     */
    @Override
    public String profileName() {
        assertValidated();
        return profileName;
    }

    /**
     * Workers
     */
    public HealthCheckConfiguration setWorkers(int workers) {
        this.workers = workers;
        return this;
    }

    /**
     * Workers
     */
    public int workers() {
        assertValidated();
        return workers;
    }

    /**
     * Time Interval
     */
    public HealthCheckConfiguration setTimeInterval(int timeInterval) {
        this.timeInterval = timeInterval;
        return this;
    }

    /**
     * Time Interval
     */
    public int timeInterval() {
        assertValidated();
        return timeInterval;
    }

    /**
     * Validate all parameters of this configuration
     *
     * @return this class instance
     * @throws IllegalArgumentException If any value is invalid
     */
    public HealthCheckConfiguration validate() throws IllegalArgumentException {
        if (id == null) {
            id = UUID.randomUUID().toString();
        }
        Objects.requireNonNull(profileName, "Profile Name");
        NumberUtil.checkPositive(workers, "Workers");
        NumberUtil.checkPositive(timeInterval, "TimeInterval");
        validated = true;
        return this;
    }

    @Override
    public String id() {
        return id;
    }

    @Override
    public boolean validated() {
        return validated;
    }
}
