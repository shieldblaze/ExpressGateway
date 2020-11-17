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
import com.shieldblaze.expressgateway.backend.events.BackendEvent;
import com.shieldblaze.expressgateway.backend.exceptions.BackendNotOnlineException;
import com.shieldblaze.expressgateway.concurrent.GlobalExecutors;
import com.shieldblaze.expressgateway.concurrent.eventstream.AsyncEventStream;
import com.shieldblaze.expressgateway.concurrent.eventstream.EventListener;
import com.shieldblaze.expressgateway.concurrent.eventstream.EventStream;

import java.util.List;
import java.util.Objects;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.stream.Collectors;

/**
 * Base class for Cluster
 */
public abstract class Cluster {

    /**
     * List of all {@linkplain Node} associated with this {@linkplain Cluster}
     */
    private final List<Node> allNodes = new CopyOnWriteArrayList<>();

    /**
     * Hostname of this {@linkplain Cluster}
     */
    private String hostname;

    /**
     * Name of this {@linkplain Cluster}
     */
    private String name;

    /**
     * Get hostname of this {@linkplain Cluster}
     *
     * @return Hostname as {@link String}
     */
    public String hostname() {
        return hostname;
    }

    /**
     * Set hostname of this {@linkplain Cluster}
     *
     * @param hostname Name as {@link String}
     * @throws IllegalArgumentException If hostname is invalid
     */
    public void hostname(String hostname) {
        this.hostname = Objects.requireNonNull(hostname, "hostname");
    }

    /**
     * Get name of this {@linkplain Cluster}
     *
     * @return Name as {@link String}
     */
    public String name() {
        return name;
    }

    /**
     * Set name of this {@linkplain Cluster}
     *
     * @param name Name as {@link String}
     */
    public void name(String name) {
        this.name = Objects.requireNonNull(name, "name");
    }

    /**
     * Add {@link Node} into this {@linkplain Cluster}
     */
    protected void addNode(Node node) {
        allNodes.add(Objects.requireNonNull(node, "backend"));
        node.cluster(this);
    }

    /**
     * Get {@linkplain List} of online {@linkplain Node} in this {@linkplain Cluster}
     */
    public List<Node> onlineBackends() {
        return allNodes.stream()
                .filter(backend -> backend.state() == State.ONLINE)
                .collect(Collectors.toList());
    }

    public Node get(int index) {
        return allNodes.get(index);
    }

    /**
     * Get {@linkplain Node} from online pool using Index
     *
     * @param index Index
     * @return {@linkplain Node} Instance if found else {@code null}
     */
    public Node online(int index) throws BackendNotOnlineException {
        Node node = allNodes.get(index);
        if (node.state() != State.ONLINE) {
            throw new BackendNotOnlineException(node);
        }
        return node;
    }

    /**
     * Get size of this {@linkplain Cluster}
     */
    public int size() {
        return allNodes.size();
    }

    /**
     * Get number of Online {@linkplain Node} in this {@linkplain Cluster}
     */
    public int online() {
        return (int) allNodes.stream().filter(backend -> backend.state() == State.ONLINE).count();
    }
}
