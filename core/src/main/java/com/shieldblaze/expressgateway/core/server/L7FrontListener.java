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
package com.shieldblaze.expressgateway.core.server;

import com.shieldblaze.expressgateway.core.concurrent.async.L4FrontListenerEvent;
import com.shieldblaze.expressgateway.core.loadbalancer.l7.L7LoadBalancer;

import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.concurrent.CompletableFuture;

public abstract class L7FrontListener {
    protected final List<CompletableFuture<L4FrontListenerEvent>> completableFutureList = new ArrayList<>();

    private L7LoadBalancer l7LoadBalancer;

    public abstract List<CompletableFuture<L4FrontListenerEvent>> start();

    public abstract CompletableFuture<Boolean> stop();

    public L7LoadBalancer getL7LoadBalancer() {
        return l7LoadBalancer;
    }

    public void setL7LoadBalancer(L7LoadBalancer l7FrontListener) {
        if (this.l7LoadBalancer != null) {
            throw new IllegalArgumentException("L7LoadBalancer is already set");
        }
        this.l7LoadBalancer = l7FrontListener;
    }

    public List<CompletableFuture<L4FrontListenerEvent>> getFutures() {
        return Collections.unmodifiableList(completableFutureList);
    }
}
