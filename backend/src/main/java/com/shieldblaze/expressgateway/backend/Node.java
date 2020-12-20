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
package com.shieldblaze.expressgateway.backend;

import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.connection.Connection;
import com.shieldblaze.expressgateway.backend.events.node.NodeEvent;
import com.shieldblaze.expressgateway.backend.events.node.NodeIdleEvent;
import com.shieldblaze.expressgateway.backend.events.node.NodeOfflineEvent;
import com.shieldblaze.expressgateway.backend.events.node.NodeOnlineEvent;
import com.shieldblaze.expressgateway.backend.exceptions.TooManyConnectionsException;
import com.shieldblaze.expressgateway.common.Math;
import com.shieldblaze.expressgateway.common.annotation.NonNull;
import com.shieldblaze.expressgateway.common.utils.Number;
import com.shieldblaze.expressgateway.healthcheck.Health;
import com.shieldblaze.expressgateway.healthcheck.HealthCheck;

import java.net.InetSocketAddress;
import java.util.Objects;
import java.util.Queue;
import java.util.concurrent.ConcurrentLinkedQueue;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicLong;

/**
 * <p> {@link Node} is the server where all requests are sent. </p>
 */
public final class Node implements Comparable<Node> {

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
     * Hash of {@link InetSocketAddress}
     */
    private final String hash;

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
    private final HealthCheck healthCheck;

    /**
     * Max Connections handled by this {@link Node}
     */
    private int maxConnections;

    public Node(Cluster cluster, InetSocketAddress socketAddress) {
        this(cluster, socketAddress, -1, null);
    }

    /**
     * Create a new {@linkplain Node} Instance
     *
     * @param socketAddress Address of this {@linkplain Node}
     */
    public Node(@NonNull Cluster cluster,
                @NonNull InetSocketAddress socketAddress,
                int maxConnections,
                HealthCheck healthCheck) {

        this.cluster = cluster;
        this.cluster.addNode(this);

        this.socketAddress = socketAddress;
        this.hash = String.valueOf(Objects.hashCode(this));
        this.healthCheck = healthCheck;

        maxConnections(maxConnections);
        state(State.ONLINE);
    }

    public Cluster cluster() {
        return cluster;
    }

    public InetSocketAddress socketAddress() {
        return socketAddress;
    }

    public int activeConnection() {

        // If active connection is initialized (value not set to 0) then return it.
        if (activeConnection0() != 0) {
            return activeConnection0();
        }

        return availableConnections.size();
    }

    public Node incBytesSent(int bytes) {
        bytesSent.addAndGet(bytes);
        return this;
    }

    public Node incBytesReceived(int bytes) {
        bytesReceived.addAndGet(bytes);
        return this;
    }

    public long bytesSent() {
        return bytesSent.get();
    }

    public long bytesReceived() {
        return bytesReceived.get();
    }

    public State state() {
        return state;
    }

    @NonNull
    public Node state(State state) {

        NodeEvent nodeEvent;

        switch (state) {
            case ONLINE:
                nodeEvent = new NodeOnlineEvent(this);
                break;
            case OFFLINE:
                nodeEvent = new NodeOfflineEvent(this);
                drainConnections();
                break;
            case IDLE:
                nodeEvent = new NodeIdleEvent(this);
                break;
            default:
                throw new IllegalArgumentException("Unknown State: " + state);
        }

        this.state = state;
        nodeEvent.trySuccess(null);
        cluster().eventPublisher().publish(nodeEvent);
        return this;
    }

    public int activeConnection0() {
        return activeConnection0.get();
    }

    public Node incActiveConnection0() {
        activeConnection0.incrementAndGet();
        return this;
    }

    public Node decActiveConnection0() {
        activeConnection0.incrementAndGet();
        return this;
    }

    public Node resetActiveConnection0() {
        activeConnection0.set(-1);
        return this;
    }

    public Health health() {
        if (healthCheck == null) {
            return Health.UNKNOWN;
        }
        return healthCheck.health();
    }

    public HealthCheck healthCheck() {
        return healthCheck;
    }

    public int maxConnections() {
        return maxConnections;
    }

    /**
     * <p> Set maximum number of connections. </p>
     * <p> Valid range: -1 to 2147483647 </p>
     * <p> Setting value to -1 will allow unlimited amount of connections. </p>
     */
    public void maxConnections(int maxConnections) {
        this.maxConnections = Number.checkRange(maxConnections, -1, Integer.MAX_VALUE, "MaxConnections");
    }

    public String hash() {
        return hash;
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
        return Math.percentage(activeConnection(), maxConnections);
    }

    /**
     * Add a {@link Connection} with this {@linkplain Node}
     */
    public Node addConnection(Connection connection) throws TooManyConnectionsException {
        // If Maximum Connection is not -1 and Number of Active connections is greater than
        // Maximum number of connections then close the connection and throw an exception.
        if (maxConnections != -1 && activeConnection() >= maxConnections) {
            connection.close();
            throw new TooManyConnectionsException(this);
        }
        activeConnections.add(connection);
        return this;
    }

    /**
     * Remove and close a {@link Connection} from this {@linkplain Node}
     */
    public Node removeConnection(Connection connection) {
        availableConnections.remove(connection);
        activeConnections.remove(connection);
        connection.close();
        return this;
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
     * Lease a connection and remove it from available active connection pool.
     */
    public Node lease0(Connection connection) {
        activeConnections.remove(connection);
        return this;
    }

    /**
     * Release a connection and add it into available active connection pool.
     */
    public Node release0(Connection connection) {
        availableConnections.add(connection);
        return this;
    }

    public Queue<Connection> activeConnections() {
        return activeConnections;
    }

    public Queue<Connection> availableConnections() {
        return availableConnections;
    }

    /**
     * Returns {@code true} if connections has reached maximum limit else {@code false}.
     */
    public boolean connectionFull() {
        if (maxConnections == -1) {
            return false;
        }
        return activeConnection() >= maxConnections;
    }

    /**
     * Close all active connection and clear available connection list
     */
    private void drainConnections() {
        activeConnections.forEach(Connection::close);
        activeConnections.clear();
        availableConnections.clear();
    }

    @Override
    public String toString() {
        return "Node{" +
                ", Cluster=" + cluster +
                ", Address=" + socketAddress +
                ", BytesSent=" + bytesSent +
                ", BytesReceived=" + bytesReceived +
                ", Connections=" + activeConnection() + "/" + maxConnections +
                ", state=" + state +
                ", healthCheck=" + healthCheck +
                '}';
    }

    @Override
    public int compareTo(Node n) {
        return hash.compareToIgnoreCase(n.hash);
    }
}
