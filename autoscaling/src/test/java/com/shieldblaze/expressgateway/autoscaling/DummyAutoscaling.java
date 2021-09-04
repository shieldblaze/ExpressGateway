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
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.List;

final class DummyAutoscaling extends Autoscaling {

    private static final Logger logger = LogManager.getLogger(DummyAutoscaling.class);

    private boolean scaleOut;
    private boolean scaleIn;
    private boolean hibernate;
    private boolean deHibernate;

    /**
     * Create a new {@link DummyAutoscaling} Instance
     *
     * @param eventStream   {@link EventStream} where events will be published
     * @param configuration {@link AutoscalingConfiguration} Instance
     */
    DummyAutoscaling(EventStream eventStream, AutoscalingConfiguration configuration) {
        super(eventStream, configuration);
    }

    @Override
    public AutoscalingScaleOutEvent scaleOut() {
        logger.debug("Scale out has been called");
        scaleOut = true;
        return null;
    }

    @Override
    public AutoscalingScaleInEvent scaleIn() {
        logger.debug("Scale in has been called");
        scaleIn = true;
        return null;
    }

    @Override
    public AutoscalingHibernateEvent hibernate() {
        logger.debug("Hibernate this server has been called");
        hibernate = true;
        return null;
    }

    @Override
    public AutoscalingDehibernateEvent dehibernate() {
        logger.debug("Dehibernate this server has been called");
        deHibernate = true;
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

    public boolean isScaleOut() {
        return scaleOut;
    }

    public boolean isScaleIn() {
        return scaleIn;
    }

    public boolean isHibernate() {
        return hibernate;
    }

    public boolean isDeHibernate() {
        return deHibernate;
    }
}
