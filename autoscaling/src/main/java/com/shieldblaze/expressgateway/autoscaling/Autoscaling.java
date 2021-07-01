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
package com.shieldblaze.expressgateway.autoscaling;

import com.shieldblaze.expressgateway.autoscaling.event.AutoscalingDehibernateEvent;
import com.shieldblaze.expressgateway.autoscaling.event.AutoscalingHibernateEvent;
import com.shieldblaze.expressgateway.autoscaling.event.AutoscalingScaleInEvent;
import com.shieldblaze.expressgateway.autoscaling.event.AutoscalingScaleOutEvent;
import com.shieldblaze.expressgateway.concurrent.eventstream.EventStream;
import com.shieldblaze.expressgateway.configuration.autoscaling.AutoscalingConfiguration;

import java.util.List;

/**
 * Base class of Autoscaling.
 */
public abstract class Autoscaling {

    private final AutoscalingConfiguration configuration;
    private final EventStream eventStream;

    /**
     * Create a new {@link Autoscaling} Instance
     *
     * @param eventStream {@link EventStream} where events will be published
     * @param configuration {@link AutoscalingConfiguration} Instance
     */
    public Autoscaling(EventStream eventStream, AutoscalingConfiguration configuration) {
        this.eventStream = eventStream;
        this.configuration = configuration;
    }

    /**
     * Scale up servers fleet by 1 (one).
     */
    public abstract AutoscalingScaleOutEvent scaleOut();

    /**
     * Scale down servers fleet by 1 (one).
     */
    public abstract AutoscalingScaleInEvent scaleIn();

    /**
     * Put a server in Hibernate state
     */
    public abstract AutoscalingHibernateEvent hibernate();

    /**
     * Pull a server out of Hibernate state
     */
    public abstract AutoscalingDehibernateEvent dehibernate();

    /**
     * List of Servers in fleet
     */
    public abstract List<AutoscalingServer> servers();

    /**
     * Current (This) Server
     */
    public abstract AutoscalingServer server();

    /**
     * {@link AutoscalingConfiguration} Instance
     */
    public AutoscalingConfiguration configuration() {
        return configuration;
    }

    /**
     * {@link EventStream} for publishing events
     */
    protected EventStream eventStream() {
        return eventStream;
    }
}
