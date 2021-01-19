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
package com.shieldblaze.expressgateway.core.loadbalancer;

import com.shieldblaze.expressgateway.core.events.L4FrontListenerStartupEvent;

import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;

public class LoadBalancerProperty {
    private String profileName;
    private L4FrontListenerStartupEvent startupEvent;

    public String profileName() {
        return profileName;
    }

    public LoadBalancerProperty profileName(String profileName) {
        this.profileName = profileName;
        return this;
    }

    public L4FrontListenerStartupEvent startupEvent() {
        return startupEvent;
    }

    public LoadBalancerProperty startupEvent(L4FrontListenerStartupEvent startupEvent) {
        this.startupEvent = startupEvent;
        return this;
    }
}
