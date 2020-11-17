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
import com.shieldblaze.expressgateway.backend.exceptions.TooManyConnectionsException;
import com.shieldblaze.expressgateway.backend.healthcheckmanager.HealthCheckManager;
import com.shieldblaze.expressgateway.backend.pool.Connection;
import com.shieldblaze.expressgateway.common.Math;
import com.shieldblaze.expressgateway.concurrent.GlobalExecutors;
import com.shieldblaze.expressgateway.common.crypto.Hasher;
import com.shieldblaze.expressgateway.healthcheck.Health;
import com.shieldblaze.expressgateway.healthcheck.HealthCheck;

import java.io.Closeable;
import java.net.InetSocketAddress;
import java.nio.ByteBuffer;
import java.util.Objects;
import java.util.Optional;
import java.util.Queue;
import java.util.concurrent.ConcurrentLinkedQueue;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.TimeUnit;

/**
 * {@link Node} is the server where all requests are sent.
 */
public class Node implements Comparable<Node>, Closeable {

    /**
     * Address of this {@link Node}
     */
    private final InetSocketAddress socketAddress;

    /**
     * Health Check Manager for this {@linkplain Node}
     */
    private final HealthCheckManager healthCheckManager;

    /**
     * Hash of {@link InetSocketAddress}
     */
    private final String hash;

    /**
     * {@linkplain Cluster} to which this {@linkplain Node} is associated
     */
    private Cluster cluster;

    /**
     * Weight of this {@link Node}
     */
    private int Weight;

    /**
     * Maximum Number Of Connections Allowed for this {@link Node}
     */
    private int maxConnections;

    /**
     * Active Number Of Connection for this {@link Node}
     */
    private int activeConnections;

    /**
     * Number of bytes written so far to this {@link Node}
     */
    private long bytesWritten = 0L;

    /**
     * Number of bytes received so far from this {@link Node}
     */
    private long bytesReceived = 0L;

    /**
     * Current State of this {@link Node}
     */
    private State state;

    /**
     * Health Check for this {@link Node}
     */
    private HealthCheck healthCheck;

    /**
     * Connections List
     */
    final Queue<Connection> connectionList = new ConcurrentLinkedQueue<>();
    private final ScheduledFuture<?> connectionCleanerFuture;

    /**
     * Create a new {@linkplain Node} Instance with {@code Weight 100}, {@code maxConnections 10000} and no Health Check
     *
     * @param socketAddress Address of this {@linkplain Node}
     */
    public Node(InetSocketAddress socketAddress) {
        this(socketAddress, 100, 10_000, null, null);
    }

    /**
     * Create a new {@linkplain Node} Instance with no Health Check
     *
     * @param socketAddress  Address of this {@linkplain Node}
     * @param Weight         Weight of this {@linkplain Node}
     * @param maxConnections Maximum Number of Connections allowed for this {@linkplain Node}
     */
    public Node(InetSocketAddress socketAddress, int Weight, int maxConnections) {
        this(socketAddress, Weight, maxConnections, null, null);
    }

    /**
     * Create a new {@linkplain Node} Instance
     *
     * @param socketAddress  Address of this {@linkplain Node}
     * @param Weight         Weight of this {@linkplain Node}
     * @param maxConnections Maximum Number of Connections allowed for this {@linkplain Node}
     * @param healthCheck    {@linkplain HealthCheck} Instance
     */
    public Node(InetSocketAddress socketAddress, int Weight, int maxConnections, HealthCheck healthCheck) {
        this(socketAddress, Weight, maxConnections, healthCheck, null);
    }

    /**
     * Create a new {@linkplain Node} Instance
     *
     * @param socketAddress      Address of this {@linkplain Node}
     * @param Weight             Weight of this {@linkplain Node}
     * @param maxConnections     Maximum Number of Connections allowed for this {@linkplain Node}
     * @param healthCheck        {@linkplain HealthCheck} Instance
     * @param healthCheckManager {@linkplain HealthCheckManager} Instance
     */
    public Node(InetSocketAddress socketAddress, int Weight, int maxConnections, HealthCheck healthCheck, HealthCheckManager healthCheckManager) {
        Objects.requireNonNull(socketAddress, "SocketAddress");

        if (Weight < 1) {
            throw new IllegalArgumentException("Weight cannot be less than 1 (one).");
        }

        if (maxConnections < 0) {
            throw new IllegalArgumentException("Maximum Connection cannot be less than 0 (Zero).");
        }

        this.state = State.ONLINE;
        this.socketAddress = socketAddress;
        this.Weight = Weight;
        this.maxConnections = maxConnections;
        this.healthCheck = healthCheck;
        this.healthCheckManager = healthCheckManager;

        if (this.healthCheck != null && this.healthCheckManager != null) {
            this.healthCheckManager.backend(this);
            this.healthCheckManager.initialize();
        }

        // Hash this backend
        ByteBuffer byteBuffer = ByteBuffer.allocate(6);
        byteBuffer.put(socketAddress.getAddress().getAddress());
        byteBuffer.putShort((short) socketAddress.getPort());
        byte[] addressAndPort = byteBuffer.array();
        byteBuffer.clear();
        this.hash = Hasher.hash(Hasher.Algorithm.SHA256, addressAndPort);

        connectionCleanerFuture = GlobalExecutors.INSTANCE.submitTaskAndRunEvery(new ConnectionCleaner(this), 1, 10, TimeUnit.MICROSECONDS);
    }

    public Cluster cluster() {
        return cluster;
    }

    public Node cluster(Cluster cluster) {
        if (this.cluster == null) {
            this.cluster = cluster;
        } else {
            throw new IllegalArgumentException("Cluster is already set");
        }
        return this;
    }

    public InetSocketAddress socketAddress() {
        return socketAddress;
    }

    public int weight() {
        return Weight;
    }

    public Node weight(int weight) {
        this.Weight = weight;
        return this;
    }

    public int activeConnections() {
        return activeConnections;
    }

    public Node incConnections() {
        activeConnections++;
        return this;
    }

    public Node decConnections() {
        activeConnections--;
        return this;
    }

    public Node incBytesWritten(int bytes) {
        bytesWritten += bytes;
        return this;
    }

    public Node incBytesWritten(long bytes) {
        bytesWritten += bytes;
        return this;
    }

    public Node incBytesReceived(int bytes) {
        bytesReceived += bytes;
        return this;
    }

    public int maxConnections() {
        return maxConnections;
    }

    public Node maxConnections(int maxConnections) {
        this.maxConnections = maxConnections;
        return this;
    }

    public long bytesWritten() {
        return bytesWritten;
    }

    public long bytesReceived() {
        return bytesReceived;
    }

    public State state() {
        return state;
    }

    public Node state(State state) {
        this.state = state;
        return this;
    }

    public HealthCheck healthCheck() {
        return healthCheck;
    }

    public Node healthCheck(HealthCheck healthCheck) {
        this.healthCheck = healthCheck;
        return this;
    }

    public Health health() {
        if (healthCheck == null) {
            return Health.UNKNOWN;
        }
        return healthCheck.health();
    }

    public String hash() {
        return hash;
    }

    public float load() {
        if (activeConnections() == 0) {
            return 0;
        }
        return Math.percentage(activeConnections, maxConnections);
    }

    /**
     * Try to lease a connection from available connections. This method automatically calls
     * {@link Connection#lease()}.
     *
     * @return {@linkplain Connection} Instance of available and active connection
     */
    public Connection lease() {
        Optional<Connection> optionalConnection = connectionList.stream()
                .filter(connection -> !connection.isInUse())
                .findAny();

        // If we've an available connection then return it.
        if (optionalConnection.isPresent()) {

            // If connection is not active, remove it and return null.
            if (!optionalConnection.get().isActive()) {
                connectionList.remove(optionalConnection.get());
                return null;
            }
            return optionalConnection.get();
        } else {
            return null;
        }
    }

    /**
     * Associate a {@link Connection} with this {@linkplain Node}
     *
     * @param connection {@link Connection} to be associated
     */
    public Node addConnection(Connection connection) throws TooManyConnectionsException {
        if (connectionList.size() > maxConnections) {
            throw new TooManyConnectionsException(this, connectionList.size(), maxConnections);
        }
        connectionList.add(connection);
        return this;
    }

    /**
     * Dissociate a {@link Connection} with from {@linkplain Node}
     *
     * @param connection {@link Connection} to be Dissociated
     */
    public Node removeConnection(Connection connection) {
        connectionList.remove(connection);
        connection.close();
        return this;
    }

    @Override
    public void close() {
        state = State.OFFLINE;
        connectionCleanerFuture.cancel(true);
        if (this.healthCheck != null && this.healthCheckManager != null) {
            healthCheckManager.shutdown();
        }
        connectionList.forEach(Connection::close);
        connectionList.clear();
    }

    @Override
    public String toString() {
        return "Backend{" +
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
    public int compareTo(Node o) {
        return hash.compareToIgnoreCase(o.hash);
    }
}
