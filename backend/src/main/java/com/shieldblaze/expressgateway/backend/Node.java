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
import com.shieldblaze.expressgateway.backend.events.node.NodeIdleEvent;
import com.shieldblaze.expressgateway.backend.events.node.NodeOfflineEvent;
import com.shieldblaze.expressgateway.backend.events.node.NodeOnlineEvent;
import com.shieldblaze.expressgateway.backend.exceptions.TooManyConnectionsException;
import com.shieldblaze.expressgateway.backend.pool.Connection;
import com.shieldblaze.expressgateway.common.Math;
import com.shieldblaze.expressgateway.common.annotation.NonNull;
import com.shieldblaze.expressgateway.common.utils.Number;
import com.shieldblaze.expressgateway.healthcheck.Health;
import com.shieldblaze.expressgateway.healthcheck.HealthCheck;

import java.net.InetSocketAddress;
import java.util.Objects;
import java.util.Optional;
import java.util.Queue;
import java.util.concurrent.ConcurrentLinkedQueue;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicLong;

/**
 * <p> {@link Node} is the server where all requests are sent. </p>
 */
public final class Node implements Comparable<Node> {

    /**
     * Connections List
     * <p>
     * (Wrapped in ConcurrentLinkedQueue due to performance issues with CopyOnWriteArrayList).
     */
    private final Queue<Connection> connections = new ConcurrentLinkedQueue<>();

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
    private final AtomicInteger activeConnection0 = new AtomicInteger(-1);

    /**
     * Current State of this {@link Node}
     */
    private State state;

    /**
     * Health Check for this {@link Node}
     */
    private final HealthCheck healthCheck;

    /**
     * Weight of this {@link Node}
     */
    private int weight;

    /**
     * Max Connections handled by this {@link Node}
     */
    private int maxConnections;

    public Node(Cluster cluster, InetSocketAddress socketAddress) {
        this(cluster, socketAddress, 100, -1, null);
    }

    /**
     * Create a new {@linkplain Node} Instance
     *
     * @param socketAddress Address of this {@linkplain Node}
     */
    public Node(@NonNull Cluster cluster,
                @NonNull InetSocketAddress socketAddress,
                int weight,
                int maxConnections,
                HealthCheck healthCheck) {

        this.cluster = cluster;
        this.socketAddress = socketAddress;
        this.hash = String.valueOf(Objects.hashCode(this));
        this.healthCheck = healthCheck;

        weight(weight);
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

        // If active connection is initialized (value not set to -1) then return it.
        if (activeConnection0() != -1) {
            return activeConnection0();
        }

        return connections.size();
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

        switch (state) {
            case ONLINE:
                cluster().eventPublisher().publish(new NodeOnlineEvent(this));
                break;
            case OFFLINE:
                cluster().eventPublisher().publish(new NodeOfflineEvent(this));
                break;
            case IDLE:
                cluster().eventPublisher().publish(new NodeIdleEvent(this));
                break;
            default:
                throw new IllegalArgumentException("Unknown State: " + state);
        }

        this.state = state;
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

    public int weight() {
        return weight;
    }

    public void weight(int weight) {
        this.weight = Number.checkPositive(weight, "Weight");
    }

    public int maxConnections() {
        return maxConnections;
    }

    public void maxConnections(int maxConnections) {
        this.maxConnections = Number.checkRange(maxConnections, -1, 1_000_000, "MaxConnections");
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
     * Try to lease a connection from available connections. This method automatically calls
     * {@link Connection#lease()}.
     *
     * @return {@linkplain Connection} Instance of available and active connection else {@code null}.
     */
    public Connection lease() {
        Optional<Connection> optionalConnection = connections.stream()
                .filter(connection -> !connection.isInUse())
                .findAny();

        // If we've an available connection then return it.
        if (optionalConnection.isPresent()) {

            // If connection is not active, remove it and return null.
            if (!optionalConnection.get().isActive()) {
                connections.remove(optionalConnection.get());
                return null;
            }
            return optionalConnection.get();
        } else {
            return null;
        }
    }

    /**
     * Add a {@link Connection} with this {@linkplain Node}
     */
    public Node addConnection(Connection connection) throws TooManyConnectionsException {
        if (maxConnections != -1 && activeConnection() > maxConnections) {
            connection.close();
            throw new TooManyConnectionsException(this, maxConnections);
        }
        connections.add(connection);
        return this;
    }

    /**
     * Remove and close a {@link Connection} from this {@linkplain Node}
     */
    public Node removeConnection(Connection connection) {
        connections.remove(connection);
        connection.close();
        return this;
    }

    public Queue<Connection> connections() {
        return connections;
    }

    private void drainConnections() {
        connections.forEach(Connection::close);
        connections.clear();
    }

    public boolean connectionFull() {
        if (maxConnections == -1) {
            return false;
        }
        return activeConnection() >= maxConnections;
    }

    @Override
    public String toString() {
        return "Node{" +
                "socketAddress=" + socketAddress +
                ", state=" + state +
                ", healthCheck=" + health() +
                '}';
    }

    @Override
    public boolean equals(Object obj) {
        if (obj instanceof Node) {
            Node node = (Node) obj;
            return hashCode() == node.hashCode();
        }
        return false;
    }

    @Override
    public int hashCode() {
        return socketAddress.hashCode();
    }

    @Override
    public int compareTo(Node n) {
        return hash.compareToIgnoreCase(n.hash);
    }
}
