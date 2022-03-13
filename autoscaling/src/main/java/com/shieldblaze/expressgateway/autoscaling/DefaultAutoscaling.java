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
package com.shieldblaze.expressgateway.autoscaling;

import com.shieldblaze.expressgateway.autoscaling.event.AutoscalingDehibernateEvent;
import com.shieldblaze.expressgateway.autoscaling.event.AutoscalingHibernateEvent;
import com.shieldblaze.expressgateway.autoscaling.event.AutoscalingScaleInEvent;
import com.shieldblaze.expressgateway.autoscaling.event.AutoscalingScaleOutEvent;
import com.shieldblaze.expressgateway.concurrent.eventstream.EventStream;
import com.shieldblaze.expressgateway.configuration.autoscaling.AutoscalingConfiguration;

import java.util.List;

public class DefaultAutoscaling extends Autoscaling {

    /**
     * Create a new {@link DefaultAutoscaling} Instance
     *
     * @param eventStream   {@link EventStream} where events will be published
     * @param configuration {@link AutoscalingConfiguration} Instance
     */
    public DefaultAutoscaling(EventStream eventStream, AutoscalingConfiguration configuration) {
        super(eventStream, configuration);
    }

    @Override
    public AutoscalingScaleOutEvent scaleOut() {
        return null;
    }

    @Override
    public AutoscalingScaleInEvent scaleIn() {
        return null;
    }

    @Override
    public AutoscalingHibernateEvent hibernate() {
        return null;
    }

    @Override
    public AutoscalingDehibernateEvent dehibernate() {
        return null;
    }

    @Override
    public List<AutoscalingServer> servers() {
        return null;
    }

    @Override
    public AutoscalingServer server() {
        return null;
    }
}
