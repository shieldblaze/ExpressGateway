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
package com.shieldblaze.expressgateway.backend.cluster;

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.State;
import com.shieldblaze.expressgateway.backend.events.node.NodeAddedEvent;
import com.shieldblaze.expressgateway.backend.events.node.NodeRemovedEvent;
import com.shieldblaze.expressgateway.backend.exceptions.LoadBalanceException;
import com.shieldblaze.expressgateway.backend.loadbalance.LoadBalance;
import com.shieldblaze.expressgateway.backend.loadbalance.Request;
import com.shieldblaze.expressgateway.backend.loadbalance.Response;
import com.shieldblaze.expressgateway.common.annotation.NonNull;
import com.shieldblaze.expressgateway.concurrent.eventstream.EventPublisher;
import com.shieldblaze.expressgateway.concurrent.eventstream.EventStream;
import com.shieldblaze.expressgateway.concurrent.eventstream.EventSubscriber;

import java.util.List;
import java.util.concurrent.CopyOnWriteArrayList;

/**
 * Base class for Cluster
 */
public abstract class Cluster {

    private final List<Node> nodes = new CopyOnWriteArrayList<>();
    private String name;

    private final EventStream eventStream;
    private final LoadBalance<?, ?, ?, ?> loadBalance;

    public Cluster(EventStream eventStream, LoadBalance<?, ?, ?, ?> loadBalance) {
        this.eventStream = eventStream;
        this.loadBalance = loadBalance;
        loadBalance.cluster(this);
    }

    /**
     * Get name of this {@linkplain Cluster}
     */
    public String name() {
        return name;
    }

    /**
     * Set name of this {@linkplain Cluster}
     */
    @NonNull
    public Cluster name(String name) {
        this.name = name;
        return this;
    }

    /**
     * Add {@link Node} into this {@linkplain Cluster}
     */
    @NonNull
    public boolean addNode(Node node) {
        for (Node n : nodes) {
            if (n.socketAddress() == node.socketAddress()) {
                return false;
            }
        }

        nodes.add(node);
        eventStream.publish(new NodeAddedEvent(node));
        return true;
    }

    public boolean removeNode(Node node) {
        boolean isFound = false;
        for (Node n : nodes) {
            if (n.socketAddress() == node.socketAddress()) {
                isFound = true;
                break;
            }
        }

        if (!isFound) {
            return false;
        }

        nodes.remove(node);
        node.state(State.OFFLINE);
        eventStream.publish(new NodeRemovedEvent(node));
        return true;
    }

    public List<Node> nodes() {
        return nodes;
    }

    /**
     * Get the next {@link Node} available to handle request.
     *
     * @throws LoadBalanceException In case of some error while generating {@linkplain Response}
     */
    @NonNull
    public Response nextNode(Request request) throws LoadBalanceException {
        return loadBalance.response(request);
    }

    public EventSubscriber eventSubscriber() {
        return eventStream.eventSubscriber();
    }

    public EventPublisher eventPublisher() {
        return eventStream.eventPublisher();
    }
}
