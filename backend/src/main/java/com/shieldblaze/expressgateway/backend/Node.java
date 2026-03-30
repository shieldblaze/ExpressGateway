/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2022 ShieldBlaze
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
import com.shieldblaze.expressgateway.backend.events.node.NodeOfflineTask;
import com.shieldblaze.expressgateway.backend.events.node.NodeOnlineTask;
import com.shieldblaze.expressgateway.backend.exceptions.TooManyConnectionsException;
import com.shieldblaze.expressgateway.common.annotation.NonNull;
import com.shieldblaze.expressgateway.common.utils.MathUtil;
import com.shieldblaze.expressgateway.common.utils.NumberUtil;
import com.shieldblaze.expressgateway.configuration.healthcheck.CircuitBreakerConfiguration;
import com.shieldblaze.expressgateway.healthcheck.Health;
import com.shieldblaze.expressgateway.healthcheck.HealthCheck;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import io.netty.channel.ChannelFuture;

import java.io.Closeable;
import java.net.InetSocketAddress;
import java.util.ArrayList;
import java.util.List;
import java.util.Queue;
import java.util.UUID;
import java.util.concurrent.ConcurrentLinkedQueue;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicLong;

/**
 * <p> {@link Node} is the server (downstream) where all requests are sent. </p>
 *
 * Use {@link NodeBuilder} to build {@link Node} Instance.
 */
public final class Node implements Comparable<Node>, Closeable {

    private static final Logger logger = LogManager.getLogger(Node.class);

    /**
     * Unique identifier of the Node
     */
    private final String ID = UUID.randomUUID().toString();

    /**
     * Active Connections Queue
     */
    private final Queue<Connection> activeConnections = new ConcurrentLinkedQueue<>();

    /**
     * CM-04: O(1) counter tracking the number of items in {@link #activeConnections}.
     * ConcurrentLinkedQueue.size() is O(n) — it traverses the entire linked list on every call.
     * At 10K+ connections per node, this becomes a measurable CPU drain on every load-balancing
     * decision, health-check poll, and metrics scrape. An AtomicInteger provides O(1) reads
     * with minimal contention via CAS.
     */
    private final AtomicInteger connectionCount = new AtomicInteger(0);

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
    private volatile State state;

    /**
     * Health Check for this {@link Node}
     */
    private HealthCheck healthCheck;

    /**
     * Circuit Breaker for this {@link Node}
     */
    private CircuitBreaker circuitBreaker;

    /**
     * Max Connections handled by this {@link Node}
     */
    private volatile int maxConnections = 10_000;

    /**
     * See {@link #addedToCluster()}
     */
    private final boolean addedToCluster;

    /**
     * Create a new Instance
     */
    @NonNull
    Node(Cluster cluster, InetSocketAddress socketAddress) throws Exception {
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
     * Get number of active connections.
     * Returns the sum of both tracking mechanisms: the connection queue
     * (managed via addConnection/removeConnection) and the secondary atomic
     * counter (managed via incActiveConnection0/decActiveConnection0).
     * <p>
     * CM-04: Uses AtomicInteger counter instead of ConcurrentLinkedQueue.size()
     * which is O(n). The counter is maintained by addConnection/removeConnection.
     */
    public int activeConnection() {
        return connectionCount.get() + activeConnection0();
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
     * Decrements the number of active connections in secondary implementation.
     * Will not go below zero to prevent counter corruption from mismatched
     * increment/decrement calls.
     */
    public Node decActiveConnection0() {
        int prev;
        do {
            prev = activeConnection0.get();
            if (prev <= 0) {
                return this;
            }
        } while (!activeConnection0.compareAndSet(prev, prev - 1));
        return this;
    }

    /**
     * Reset the number of active connections in secondary implementation
     */
    public Node resetActiveConnection0() {
        activeConnection0.set(0);
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
     * Initialize the circuit breaker for this Node using the provided configuration.
     * If the configuration is disabled, the circuit breaker will still be created
     * but will always allow requests through.
     */
    @NonNull
    public void circuitBreaker(CircuitBreakerConfiguration config) {
        this.circuitBreaker = new CircuitBreaker(config);
    }

    /**
     * Returns the {@link CircuitBreaker} for this Node, or {@code null} if not configured.
     */
    public CircuitBreaker circuitBreaker() {
        return circuitBreaker;
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
        this.maxConnections = NumberUtil.checkInRange(maxConnections, 1, Integer.MAX_VALUE, "MaxConnections");
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
     * Mark this {@link Node} as offline.
     */
    public boolean markOffline() {
        if (state != State.MANUAL_OFFLINE) {
            state(State.MANUAL_OFFLINE);
            cluster.eventStream().publish(new NodeOfflineTask(this));
            return true;
        }
        return false;
    }

    /**
     * Mark this {@link Node} as Online from {@link #markOffline()}
     */
    public void markOnline() {
        state(State.ONLINE);
        cluster.eventStream().publish(new NodeOnlineTask(this));
    }

    /**
     * Add a {@link Connection} with this {@linkplain Node}
     *
     * <p>LB-F4: The connectionFull() check and the queue insertion are performed
     * atomically inside the synchronized block to eliminate the TOCTOU race where
     * two threads could both pass the check and exceed maxConnections.</p>
     *
     * <p>Uses {@code synchronized} instead of {@link java.util.concurrent.locks.ReentrantLock}
     * because virtual threads (JDK 21+) unmount from carrier threads when blocking on
     * {@code synchronized}, but {@code ReentrantLock.lock()} pins the carrier thread.</p>
     */
    public synchronized void addConnection(Connection connection) throws TooManyConnectionsException {
        if (connectionFull()) {
            connection.close();
            throw new TooManyConnectionsException(this);
        }
        if (state != State.ONLINE) {
            throw new IllegalStateException("Node is not online");
        }
        activeConnections.add(connection);
        connectionCount.incrementAndGet();
    }

    /**
     * Remove and close a {@link Connection} from this {@linkplain Node}
     */
    public synchronized void removeConnection(Connection connection) {
        if (activeConnections.remove(connection)) {
            connectionCount.decrementAndGet();
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
     * Grace period in seconds before forcibly closing HTTP/2 connections
     * after GOAWAY has been sent. Per RFC 9113 Section 6.8, the peer
     * should finish in-flight streams within this window. 5 seconds is
     * consistent with Nginx's default http2_idle_timeout and what
     * envoy uses for its drain period.
     */
    private static final long GOAWAY_GRACE_PERIOD_SECONDS = 5;

    /**
     * Drain all active connections with graceful shutdown for HTTP/2.
     * <p>
     * For HTTP/2 connections (per RFC 9113 Section 6.8): sends a GOAWAY frame
     * with NO_ERROR to signal the peer to stop sending new streams, then
     * schedules a delayed close after {@link #GOAWAY_GRACE_PERIOD_SECONDS}
     * to allow in-flight requests to complete.
     * <p>
     * For non-HTTP/2 connections: closes immediately.
     */
    public void drainConnections() {
        // Snapshot and clear the queue atomically to prevent double-close
        // if drainConnections() is called concurrently (e.g., close() racing
        // with a health-check-triggered drain).
        // Atomically drain: snapshot, clear, and reset counter while holding
        // the queue as a monitor to prevent concurrent add/remove from corrupting
        // the counter (e.g., an addConnection between clear() and set(0)).
        List<Connection> snapshot;
        synchronized (this) {
            snapshot = new ArrayList<>(activeConnections);
            activeConnections.clear();
            connectionCount.set(0);
        }

        for (Connection connection : snapshot) {
            if (connection.isHttp2()) {
                // Send GOAWAY with NO_ERROR, then schedule a forced close
                // after the grace period to clean up lingering streams.
                ChannelFuture goawayFuture = connection.sendGoaway();
                if (goawayFuture != null) {
                    goawayFuture.addListener(future -> {
                        if (!future.isSuccess()) {
                            logger.warn("Failed to send GOAWAY to {}: {}",
                                    connection, future.cause() != null ? future.cause().getMessage() : "unknown");
                            // GOAWAY send failed — close immediately, no point waiting.
                            connection.close();
                        } else {
                            // Schedule a delayed close to allow in-flight requests to finish.
                            // We use the channel's EventLoop so the close runs on the correct
                            // I/O thread, avoiding cross-thread channel operations.
                            goawayFuture.channel().eventLoop().schedule(
                                    connection::close,
                                    GOAWAY_GRACE_PERIOD_SECONDS,
                                    TimeUnit.SECONDS
                            );
                        }
                    });
                } else {
                    // sendGoaway() returned null — channel is already inactive or not H2.
                    // Fall through to immediate close.
                    connection.close();
                }
            } else {
                connection.close();
            }
        }
    }

    @Override
    public String toString() {
        return '{' +
                "Cluster=" + cluster +
                ", Address=" + socketAddress +
                ", BytesSent=" + bytesSent +
                ", BytesReceived=" + bytesReceived +
                ", Connections=" + activeConnection() + '/' + maxConnections() +
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
        return socketAddress.hashCode();
    }

    @Override
    public boolean equals(Object obj) {
        if (this == obj) {
            return true;
        }
        return obj instanceof Node node && socketAddress.equals(node.socketAddress);
    }

    /**
     * <p> Close all active connections and set {@linkplain State} to {@linkplain State#OFFLINE}. </p>
     *
     * <p> This method is called by {@link Cluster#removeNode(Node)} when this {@link Node}
     * is being removed from the cluster. </p>
     */
    @Override
    public void close() {
        try {
            logger.info("Closing Node: {} from Cluster: {}", this, cluster);

            state(State.OFFLINE);
            drainConnections();
            cluster.removeNode(this);

            logger.info("Successfully closed Node: {} from Cluster: {}", this, cluster);
        } catch (Exception ex) {
            logger.error("Failed to close Node: {} from Cluster: {}", this, cluster);
            throw ex;
        }
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
        jsonObject.addProperty("BytesSent", bytesSent.get());
        jsonObject.addProperty("BytesReceived", bytesReceived.get());
        jsonObject.addProperty("State", state.toString());
        jsonObject.addProperty("Health", health().toString());
        jsonObject.addProperty("AddedToCluster", addedToCluster);
        return jsonObject;
    }
}
