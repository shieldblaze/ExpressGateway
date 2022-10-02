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
package com.shieldblaze.expressgateway.configuration.eventloop;

import com.fasterxml.jackson.annotation.JsonIgnore;
import com.fasterxml.jackson.annotation.JsonProperty;
import com.shieldblaze.expressgateway.common.utils.NumberUtil;
import com.shieldblaze.expressgateway.configuration.Configuration;
import dev.morphia.annotations.Entity;
import dev.morphia.annotations.Id;
import dev.morphia.annotations.Property;
import dev.morphia.annotations.Transient;

import java.util.UUID;

/**
 * Configuration for {@link EventLoopConfiguration}
 */
@Entity(value = "EventLoop", useDiscriminator = false)
public final class EventLoopConfiguration implements Configuration<EventLoopConfiguration> {

    @Id
    @JsonProperty
    private String id;

    @Property
    @JsonProperty
    private int parentWorkers;

    @Property
    @JsonProperty
    private int childWorkers;

    @Transient
    @JsonIgnore
    private boolean validated;

    public static final EventLoopConfiguration DEFAULT = new EventLoopConfiguration();

    static {
        DEFAULT.id = "default";
        DEFAULT.parentWorkers = Runtime.getRuntime().availableProcessors();
        DEFAULT.childWorkers = DEFAULT.parentWorkers * 2;
        DEFAULT.validated = true;
    }

    /**
     * Parent workers
     */
    public EventLoopConfiguration setParentWorkers(int parentWorkers) {
        this.parentWorkers = parentWorkers;
        return this;
    }

    /**
     * Parent workers
     */
    public int parentWorkers() {
        assertValidated();
        return parentWorkers;
    }

    /**
     * Child workers
     */
    public EventLoopConfiguration setChildWorkers(int childWorkers) {
        this.childWorkers = childWorkers;
        return this;
    }

    /**
     * Child workers
     */
    public int childWorkers() {
        assertValidated();
        return childWorkers;
    }

    /**
     * Validate all parameters of this configuration
     *
     * @return this class instance
     * @throws IllegalArgumentException If any value is invalid
     */
    public EventLoopConfiguration validate() throws IllegalArgumentException {
        if (id == null) {
            id = UUID.randomUUID().toString();
        }
        NumberUtil.checkPositive(parentWorkers, "Parent Workers");
        NumberUtil.checkPositive(childWorkers, "Child Workers");
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
