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
package com.shieldblaze.expressgateway.backend;

import com.google.gson.JsonObject;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.events.node.NodeOfflineEvent;
import com.shieldblaze.expressgateway.backend.events.node.NodeOnlineEvent;
import com.shieldblaze.expressgateway.backend.exceptions.TooManyConnectionsException;
import com.shieldblaze.expressgateway.common.utils.MathUtil;
import com.shieldblaze.expressgateway.common.annotation.NonNull;
import com.shieldblaze.expressgateway.common.utils.NumberUtil;
import com.shieldblaze.expressgateway.healthcheck.Health;
import com.shieldblaze.expressgateway.healthcheck.HealthCheck;

import java.io.Closeable;
import java.net.InetSocketAddress;
import java.net.UnknownHostException;
import java.util.Queue;
import java.util.UUID;
import java.util.concurrent.ConcurrentLinkedQueue;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicLong;

/**
 * <p> {@link Node} is the server where all requests are sent. </p>
 *
 * Use {@link NodeBuilder} to build {@link Node} Instance.
 */
public final class Node implements Comparable<Node>, Closeable {

    /**
     * Unique identifier of the Node
     */
    private final String ID = UUID.randomUUID().toString();

    // Cache the hashCode
    private final int hashCode = ID.hashCode();

    /**
     * Available Connections Queue
     */
    private final Queue<Connection> availableConnections = new ConcurrentLinkedQueue<>();

    /**
     * Active Connections Queue
     */
    private final Queue<Connection> activeConnections = new ConcurrentLinkedQueue<>();

    /**
     * Address of this {@link Node}
     */
    private final InetSocketAddress socketAddress;

    /**
     * {@linkplain Cluster} to which this {@linkplain Node} is associated
     */
    private final Cluster cluster;

    /**
     * Number of bytes sent so far to this {@link Node}
     */
    private final AtomicLong bytesSent = new AtomicLong();

    /**
     * Number of bytes received so far from this {@link Node}
     */
    private final AtomicLong bytesReceived = new AtomicLong();

    /**
     * Active Connection secondary implementation
     */
    private final AtomicInteger activeConnection0 = new AtomicInteger(0);

    /**
     * Current State of this {@link Node}
     */
    private State state;

    /**
     * Health Check for this {@link Node}
     */
    private HealthCheck healthCheck;

    /**
     * Max Connections handled by this {@link Node}
     */
    private int maxConnections = 10_000;

    /**
     * See {@link #addedToCluster()}
     */
    private final boolean addedToCluster;

    /**
     * Create a new Instance
     */
    @NonNull
    Node(Cluster cluster, InetSocketAddress socketAddress) throws UnknownHostException {
        this.socketAddress = socketAddress;
        this.cluster = cluster;
        addedToCluster = this.cluster.addNode(this);

        state(State.ONLINE);
    }

    /**
     * Returns the associated {@link Cluster}
     */
    public Cluster cluster() {
        return cluster;
    }

    /**
     * IP and Port of this {@link Node}
     */
    public InetSocketAddress socketAddress() {
        return socketAddress;
    }

    /**
     * Get number of active connections
     */
    public int activeConnection() {

        // If active connection is initialized (value not set to 0) then return it.
        if (activeConnection0() != 0) {
            return activeConnection0();
        }

        return activeConnections.size();
    }

    /**
     * Increment bytes sent to this Node
     *
     * @param bytes Number of bytes to increment
     */
    void incBytesSent(int bytes) {
        bytesSent.addAndGet(bytes);
    }

    /**
     * Increment bytes received from this Node
     *
     * @param bytes Number of bytes to increment
     */
    void incBytesReceived(int bytes) {
        bytesReceived.addAndGet(bytes);
    }

    /**
     * Returns Number of Bytes Sent
     */
    public long bytesSent() {
        return bytesSent.get();
    }

    /**
     * Returns Number of Bytes Received
     */
    public long bytesReceived() {
        return bytesReceived.get();
    }

    /**
     * Returns the current {@link State} of this Node
     */
    public State state() {
        return state;
    }

    @NonNull
    public Node state(State state) {
        this.state = state;
        return this;
    }

    /**
     * Returns Number of active connections in secondary implementation
     */
    public int activeConnection0() {
        return activeConnection0.get();
    }

    /**
     * Increments the number of active connections in secondary implementation
     */
    public void incActiveConnection0() {
        activeConnection0.incrementAndGet();
    }

    /**
     * Decrements the number of active connections in secondary implementation
     */
    public Node decActiveConnection0() {
        activeConnection0.incrementAndGet();
        return this;
    }

    /**
     * Reset the number of active connections in secondary implementation
     */
    public Node resetActiveConnection0() {
        activeConnection0.set(-1);
        return this;
    }

    /**
     * Returns {@link Health} of this Node
     */
    public Health health() {
        // If healthCheck is null then return UNKNOWN.
        if (healthCheck == null) {
            return Health.UNKNOWN;
        }
        return healthCheck.health();
    }

    @NonNull
    public void healthCheck(HealthCheck healthCheck) {
        this.healthCheck = healthCheck;
    }

    public HealthCheck healthCheck() {
        return healthCheck;
    }

    /**
     * Returns the Number of Maximum Connection
     */
    public int maxConnections() {
        return maxConnections;
    }

    /**
     * <p> Set maximum number of connections. </p>
     * <p> Valid range: 1 to 2147483647 </p>
     */
    public void maxConnections(int maxConnections) {
        this.maxConnections = NumberUtil.checkRange(maxConnections, 1, Integer.MAX_VALUE, "MaxConnections");
    }

    /**
     * Returns {@code true} if this {@link Node} has been successfully added
     * to a {@link Cluster} else {@code false}.
     */
    public boolean addedToCluster() {
        return addedToCluster;
    }

    public String id() {
        return ID;
    }

    /**
     * Get load factor of this {@link Node}
     */
    public float load() {
        // If number of active connections is 0 (zero) or max connections is -1 (negative one)
        // then return 0 (zero) because we cannot get the percentage.
        if (activeConnection() == 0 || maxConnections == -1) {
            return 0;
        }
        return MathUtil.percentage(activeConnection(), maxConnections);
    }

    /**
     * Mark this {@link Node} as manually offline.
     */
    public boolean markManualOffline() {
        if (state != State.MANUAL_OFFLINE) {
            state(State.MANUAL_OFFLINE);
            cluster.eventStream().publish(new NodeOfflineEvent(this));
            return true;
        }
        return false;
    }

    /**
     * Mark this {@link Node} as Online from {@link #markManualOffline()}
     */
    public void unmarkManualOffline() {
        state(State.ONLINE);
        cluster.eventStream().publish(new NodeOnlineEvent(this));
    }

    /**
     * Add a {@link Connection} with this {@linkplain Node}
     */
    public void addConnection(Connection connection) throws TooManyConnectionsException, IllegalStateException {
        // If Maximum Connection is not -1 and Number of Active connections is greater than
        // Maximum number of connections then close the connection and throw an exception.
        if (connectionFull()) {
            connection.close();
            throw new TooManyConnectionsException(this);
        } else if (state != State.ONLINE) {
            throw new IllegalStateException("Node is not online");
        }
        activeConnections.add(connection);
    }

    /**
     * Remove and close a {@link Connection} from this {@linkplain Node}
     */
    public void removeConnection(Connection connection) {
        availableConnections.remove(connection);
        activeConnections.remove(connection);
        connection.close();
    }

    /**
     * Try to lease an available active connection.
     *
     * @return {@link Connection} if an available active connection is available else {@code null}
     */
    public Connection tryLease() {
        return availableConnections.poll();
    }

    /**
     * Release a connection and add it into available active connection pool.
     */
    public void release0(Connection connection) {
        // Don't add duplicate connection
        if (!availableConnections.contains(connection)) {
            availableConnections.add(connection);
        }
    }

    /**
     * Returns {@code true} if connections has reached maximum limit else {@code false}.
     */
    public boolean connectionFull() {
        // -1 means unlimited connections.
        if (maxConnections == -1) {
            return false;
        }
        return activeConnection() >= maxConnections;
    }

    /**
     * Drain all active connection
     */
    public void drainConnections() {
        activeConnections.forEach(Connection::close);
        activeConnections.clear();
        availableConnections.clear();
    }

    @Override
    public String toString() {
        return '{' +
                "Cluster=" + cluster +
                ", Address=" + socketAddress +
                ", BytesSent=" + bytesSent +
                ", BytesReceived=" + bytesReceived +
                ", Connections=" + activeConnection() + "/" + maxConnections() +
                ", state=" + state +
                ", health=" + health() +
                '}';
    }

    @Override
    public int compareTo(Node node) {
        return ID.compareToIgnoreCase(node.ID);
    }

    @Override
    public int hashCode() {
        return hashCode;
    }

    @Override
    public boolean equals(Object obj) {
        if (obj instanceof Node) {
            Node node = (Node) obj;
            return socketAddress == node.socketAddress;
        }
        return false;
    }

    /**
     * <p> Close all active connections and set {@linkplain State} to {@linkplain State#OFFLINE}. </p>
     *
     * <p> This method is called by {@link Cluster#removeNode(Node)} when this {@link Node}
     * is being removed from the cluster. </p>
     */
    @Override
    public void close() {
        state(State.OFFLINE);
        drainConnections();
        cluster.removeNode(this);
    }

    /**
     * Convert Node data into {@link JsonObject}
     * @return {@link JsonObject} Instance
     */
    public JsonObject toJson() {
        JsonObject jsonObject = new JsonObject();
        jsonObject.addProperty("ID", id());
        jsonObject.addProperty("SocketAddress", socketAddress.toString());
        jsonObject.addProperty("Connections", activeConnection() + "/" + maxConnections());
        jsonObject.addProperty("BytesSent", bytesSent);
        jsonObject.addProperty("BytesReceived", bytesReceived);
        jsonObject.addProperty("State", state.toString());
        jsonObject.addProperty("Health", health().toString());
        jsonObject.addProperty("AddedToCluster", addedToCluster);
        return jsonObject;
    }
}
