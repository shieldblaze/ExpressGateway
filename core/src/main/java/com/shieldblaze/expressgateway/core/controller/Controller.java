/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020 ShieldBlaze
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
package com.shieldblaze.expressgateway.core.controller;

import com.shieldblaze.expressgateway.backend.events.node.NodeEvent;
import com.shieldblaze.expressgateway.concurrent.event.Event;
import com.shieldblaze.expressgateway.concurrent.eventstream.EventListener;
import com.shieldblaze.expressgateway.configuration.controller.ControllerConfiguration;

import java.util.concurrent.Executor;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.ScheduledThreadPoolExecutor;

@SuppressWarnings("rawtypes")
public class Controller implements EventListener {

    final ExecutorService workers;
    final ScheduledExecutorService loopWorkers;

    final ControllerConfiguration configuration;
    private final NodeHandler nodeHandler;

    public Controller(ControllerConfiguration controllerConfiguration) {
        this.configuration = controllerConfiguration;

        // Executors
        workers = Executors.newFixedThreadPool(controllerConfiguration.workers());
        loopWorkers = Executors.newScheduledThreadPool(controllerConfiguration.workers());

        this.nodeHandler = new NodeHandler(this);
    }

    @Override
    public void accept(Event event) {
        if (event instanceof NodeEvent) {
            nodeHandler.handleEvent((NodeEvent) event);
        }
    }

    public void shutdown() {
        workers.shutdown();
        loopWorkers.shutdown();
    }
}
