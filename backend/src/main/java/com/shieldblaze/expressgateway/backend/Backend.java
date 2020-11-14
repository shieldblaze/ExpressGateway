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
import com.shieldblaze.expressgateway.common.concurrent.GlobalExecutors;
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
 * {@link Backend} is the server where all requests are sent.
 */
public class Backend implements Comparable<Backend>, Closeable {

    /**
     * Address of this {@link Backend}
     */
    private final InetSocketAddress socketAddress;

    /**
     * Health Check Manager for this {@linkplain Backend}
     */
    private final HealthCheckManager healthCheckManager;

    /**
     * Hash of {@link InetSocketAddress}
     */
    private final String hash;

    /**
     * {@linkplain Cluster} to which this {@linkplain Backend} is associated
     */
    private Cluster cluster;

    /**
     * Weight of this {@link Backend}
     */
    private int Weight;

    /**
     * Maximum Number Of Connections Allowed for this {@link Backend}
     */
    private int maxConnections;

    /**
     * Active Number Of Connection for this {@link Backend}
     */
    private int activeConnections;

    /**
     * Number of bytes written so far to this {@link Backend}
     */
    private long bytesWritten = 0L;

    /**
     * Number of bytes received so far from this {@link Backend}
     */
    private long bytesReceived = 0L;

    /**
     * Current State of this {@link Backend}
     */
    private State state;

    /**
     * Health Check for this {@link Backend}
     */
    private HealthCheck healthCheck;

    /**
     * Connections List
     */
    final Queue<Connection> connectionList = new ConcurrentLinkedQueue<>();
    private final ScheduledFuture<?> connectionCleanerFuture;

    /**
     * Create a new {@linkplain Backend} Instance with {@code Weight 100}, {@code maxConnections 10000} and no Health Check
     *
     * @param socketAddress Address of this {@linkplain Backend}
     */
    public Backend(InetSocketAddress socketAddress) {
        this(socketAddress, 100, 10_000, null, null);
    }

    /**
     * Create a new {@linkplain Backend} Instance with no Health Check
     *
     * @param socketAddress  Address of this {@linkplain Backend}
     * @param Weight         Weight of this {@linkplain Backend}
     * @param maxConnections Maximum Number of Connections allowed for this {@linkplain Backend}
     */
    public Backend(InetSocketAddress socketAddress, int Weight, int maxConnections) {
        this(socketAddress, Weight, maxConnections, null, null);
    }

    /**
     * Create a new {@linkplain Backend} Instance
     *
     * @param socketAddress  Address of this {@linkplain Backend}
     * @param Weight         Weight of this {@linkplain Backend}
     * @param maxConnections Maximum Number of Connections allowed for this {@linkplain Backend}
     * @param healthCheck    {@linkplain HealthCheck} Instance
     */
    public Backend(InetSocketAddress socketAddress, int Weight, int maxConnections, HealthCheck healthCheck) {
        this(socketAddress, Weight, maxConnections, healthCheck, null);
    }

    /**
     * Create a new {@linkplain Backend} Instance
     *
     * @param socketAddress      Address of this {@linkplain Backend}
     * @param Weight             Weight of this {@linkplain Backend}
     * @param maxConnections     Maximum Number of Connections allowed for this {@linkplain Backend}
     * @param healthCheck        {@linkplain HealthCheck} Instance
     * @param healthCheckManager {@linkplain HealthCheckManager} Instance
     */
    public Backend(InetSocketAddress socketAddress, int Weight, int maxConnections, HealthCheck healthCheck, HealthCheckManager healthCheckManager) {
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
            this.healthCheckManager.setBackend(this);
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

    public Cluster getCluster() {
        return cluster;
    }

    public void setCluster(Cluster cluster) {
        if (this.cluster == null) {
            this.cluster = cluster;
        } else {
            throw new IllegalArgumentException("Cluster is already set");
        }
    }

    public InetSocketAddress getSocketAddress() {
        return socketAddress;
    }

    public int getWeight() {
        return Weight;
    }

    public void setWeight(int weight) {
        this.Weight = weight;
    }

    public int getActiveConnections() {
        return activeConnections;
    }

    public void incConnections() {
        activeConnections++;
    }

    public void decConnections() {
        activeConnections--;
    }

    public void incBytesWritten(int bytes) {
        bytesWritten += bytes;
    }

    public void incBytesWritten(long bytes) {
        bytesWritten += bytes;
    }

    public void incBytesReceived(int bytes) {
        bytesReceived += bytes;
    }

    public int getMaxConnections() {
        return maxConnections;
    }

    public void setMaxConnections(int maxConnections) {
        this.maxConnections = maxConnections;
    }

    public long getBytesWritten() {
        return bytesWritten;
    }

    public long getBytesReceived() {
        return bytesReceived;
    }

    public State getState() {
        return state;
    }

    public void setState(State state) {
        this.state = state;
    }

    public HealthCheck getHealthCheck() {
        return healthCheck;
    }

    public void setHealthCheck(HealthCheck healthCheck) {
        this.healthCheck = healthCheck;
    }

    public Health getHealth() {
        if (healthCheck == null) {
            return Health.UNKNOWN;
        }
        return healthCheck.health();
    }

    public String getHash() {
        return hash;
    }

    public float load() {
        if (getActiveConnections() == 0) {
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
     * Associate a {@link Connection} with this {@linkplain Backend}
     *
     * @param connection {@link Connection} to be associated
     */
    public void addConnection(Connection connection) throws TooManyConnectionsException {
        if (connectionList.size() > maxConnections) {
            throw new TooManyConnectionsException(this, connectionList.size(), maxConnections);
        }
        connectionList.add(connection);
    }

    /**
     * Dissociate a {@link Connection} with from {@linkplain Backend}
     *
     * @param connection {@link Connection} to be Dissociated
     */
    public void removeConnection(Connection connection) {
        connectionList.remove(connection);
        connection.close();
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
                ", healthCheck=" + getHealth() +
                '}';
    }

    @Override
    public boolean equals(Object obj) {
        if (obj instanceof Backend) {
            Backend backend = (Backend) obj;
            return hashCode() == backend.hashCode();
        }
        return false;
    }

    @Override
    public int hashCode() {
        return socketAddress.hashCode();
    }

    @Override
    public int compareTo(Backend o) {
        return hash.compareToIgnoreCase(o.hash);
    }
}
