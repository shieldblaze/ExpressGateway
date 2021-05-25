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

import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.common.annotation.NonNull;
import com.shieldblaze.expressgateway.concurrent.GlobalExecutors;
import com.shieldblaze.expressgateway.concurrent.eventstream.EventStream;
import com.shieldblaze.expressgateway.configuration.CoreConfiguration;
import com.shieldblaze.expressgateway.configuration.tls.TLSConfiguration;
import com.shieldblaze.expressgateway.core.ConnectionTracker;
import com.shieldblaze.expressgateway.core.EventLoopFactory;
import com.shieldblaze.expressgateway.core.L4FrontListener;
import com.shieldblaze.expressgateway.core.PooledByteBufAllocator;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerShutdownEvent;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStartupEvent;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStopEvent;
import io.netty.buffer.ByteBufAllocator;
import io.netty.channel.ChannelHandler;

import java.net.InetSocketAddress;
import java.util.Map;
import java.util.Objects;
import java.util.UUID;
import java.util.concurrent.ConcurrentSkipListMap;
import java.util.concurrent.atomic.AtomicInteger;

/**
 * {@link L4LoadBalancer} holds base functions for a L4-Load Balancer.
 */
public abstract class L4LoadBalancer {

    public final String ID = UUID.randomUUID().toString();

    private static final AtomicInteger COUNTER = new AtomicInteger(0);
    private String name = "L4LoadBalancer#" + COUNTER.incrementAndGet();

    private final EventStream eventStream;
    private final InetSocketAddress bindAddress;
    private final L4FrontListener l4FrontListener;
    private final Map<String, Cluster> clusterMap = new ConcurrentSkipListMap<>();
    private final CoreConfiguration coreConfiguration;
    private final TLSConfiguration tlsForServer;
    private final TLSConfiguration tlsForClient;
    private final ChannelHandler channelHandler;

    private final ByteBufAllocator byteBufAllocator;
    private final EventLoopFactory eventLoopFactory;

    private L4FrontListenerStartupEvent l4FrontListenerStartupEvent;
    private final ConnectionTracker connectionTracker = new ConnectionTracker();

    /**
     * @param name              Name of this Load Balancer
     * @param bindAddress       {@link InetSocketAddress} on which {@link L4FrontListener} will bind and listen.
     * @param l4FrontListener   {@link L4FrontListener} for listening traffic
     * @param coreConfiguration {@link CoreConfiguration} to be applied
     * @param tlsForServer      {@link TLSConfiguration} for Server
     * @param tlsForClient      {@link TLSConfiguration} for Client
     * @param channelHandler    {@link ChannelHandler} to use for handling traffic
     * @throws NullPointerException If a required parameter if {@code null}
     */
    public L4LoadBalancer(String name,
                          @NonNull InetSocketAddress bindAddress,
                          @NonNull L4FrontListener l4FrontListener,
                          @NonNull CoreConfiguration coreConfiguration,
                          TLSConfiguration tlsForServer,
                          TLSConfiguration tlsForClient,
                          ChannelHandler channelHandler) {

        if (name != null && !name.isEmpty()) {
            this.name = name;
        }

        this.bindAddress = bindAddress;
        this.l4FrontListener = l4FrontListener;
        this.coreConfiguration = coreConfiguration;
        this.eventStream = coreConfiguration.eventStreamConfiguration().newEventStream();
        this.tlsForServer = tlsForServer;
        this.tlsForClient = tlsForClient;
        this.channelHandler = channelHandler;

        this.byteBufAllocator = new PooledByteBufAllocator(coreConfiguration.bufferConfiguration()).Instance();
        this.eventLoopFactory = new EventLoopFactory(coreConfiguration);

        l4FrontListener.l4LoadBalancer(this);
    }

    /**
     * Name of this L4 Load Balancer
     */
    public String name() {
        return name;
    }

    /**
     * Start L4 Load Balancer
     */
    public L4FrontListenerStartupEvent start() {
        l4FrontListenerStartupEvent = l4FrontListener.start();
        return l4FrontListenerStartupEvent;
    }

    /**
     * Stop L4 Load Balancer and it's child operations and services.
     */
    public L4FrontListenerStopEvent stop() {
        return l4FrontListener.stop();
    }

    public L4FrontListenerShutdownEvent shutdown() {
        L4FrontListenerShutdownEvent event = l4FrontListener.shutdown();

        // Close EventStream when stop event has finished.
        event.future().whenCompleteAsync((_Void, throwable) -> eventStream().close(), GlobalExecutors.INSTANCE.executorService());
        return event;
    }

    /**
     * Get the {@link EventStream} of this Load Balancer
     */
    public EventStream eventStream() {
        return eventStream;
    }

    /**
     * Get {@link InetSocketAddress} on which {@link L4FrontListener} is bind.
     */
    public InetSocketAddress bindAddress() {
        return bindAddress;
    }

    /**
     * Get {@link Cluster} which is being Load Balanced for specific Hostname
     *
     * @param hostname FQDN Hostname
     * @throws NullPointerException If {@link Cluster} is not found
     */
    @NonNull
    public Cluster cluster(String hostname) {
        Cluster cluster = clusterMap.get(hostname);
        if (cluster == null) {
            throw new NullPointerException("Cluster not found with Hostname: " + hostname);
        }
        return cluster;
    }

    /**
     * Get all {@link Cluster}
     */
    public Map<String, Cluster> clusters() {
        return clusterMap;
    }

    /**
     * Set the default {@link Cluster}
     */
    public void defaultCluster(Cluster cluster) {
        mapCluster("DEFAULT", cluster);
    }

    /**
     * Get the default {@link Cluster}
     */
    public Cluster defaultCluster() {
        return cluster("DEFAULT");
    }

    /**
     * Add new mapping of Cluster with Hostname
     *
     * @param hostname Fully qualified Hostname and Port if non-default port is used
     * @param cluster  {@link Cluster} to be mapped
     */
    public void mapCluster(String hostname, Cluster cluster) {
        Objects.requireNonNull(hostname, "Hostname");
        Objects.requireNonNull(cluster, "Cluster");

        cluster.useMainEventStream(eventStream); // Set EventStream
        clusterMap.put(hostname, cluster);
    }

    /**
     * Remap a {@link Cluster} from old hostname to new hostname.
     *
     * @param oldHostname Old Hostname
     * @param newHostname New Hostname
     */
    public void remapCluster(String oldHostname, String newHostname) {
        Objects.requireNonNull(oldHostname, "OldHostname");
        Objects.requireNonNull(newHostname, "NewHostname");

        Cluster cluster = clusterMap.remove(oldHostname);
        if (cluster == null) {
            throw new NullPointerException("Cluster not found with Hostname: " + oldHostname);
        }

        clusterMap.put(newHostname, cluster);
    }

    /**
     * Remove a Cluster from mapping
     *
     * @param hostname Hostname of the Cluster
     * @return Returns {@link Boolean#TRUE} if removal was successful else {@link Boolean#FALSE}
     */
    public boolean removeCluster(String hostname) {
        Cluster cluster = clusterMap.remove(hostname);
        if (cluster == null) {
            return false;
        }
        cluster.close();
        return true;
    }

    /**
     * Get {@link CoreConfiguration} which is applied
     */
    public CoreConfiguration coreConfiguration() {
        return coreConfiguration;
    }

    /**
     * Get {@link TLSConfiguration} for Server
     */
    public TLSConfiguration tlsForServer() {
        return tlsForServer;
    }

    /**
     * Get {@link TLSConfiguration} for Client
     */
    public TLSConfiguration tlsForClient() {
        return tlsForClient;
    }

    /**
     * Get {@link ChannelHandler} used for handling traffic
     */
    public ChannelHandler channelHandler() {
        return channelHandler;
    }

    /**
     * Get {@link ByteBufAllocator} created from {@link PooledByteBufAllocator}
     */
    public ByteBufAllocator byteBufAllocator() {
        return byteBufAllocator;
    }

    /**
     * Get {@link EventLoopFactory} being used
     */
    public EventLoopFactory eventLoopFactory() {
        return eventLoopFactory;
    }

    /**
     * Get {@link ConnectionTracker} Handler
     */
    public ConnectionTracker connectionTracker() {
        return connectionTracker;
    }

    /**
     * Return the Type of Load Balancer
     */
    public abstract String type();

    /**
     * Convert Load Balancer data into {@link JsonObject}
     *
     * @return {@link JsonObject} Instance
     */
    public JsonObject toJson() {
        JsonObject jsonObject = new JsonObject();

        String state;
        if (l4FrontListenerStartupEvent.isFinished()) {
            if (l4FrontListenerStartupEvent.isSuccess()) {
                state = "Running";
            } else {
                state = "Failed; " + l4FrontListenerStartupEvent.cause().getMessage();
            }
        } else {
            state = "Pending";
        }

        jsonObject.addProperty("ID", ID);
        jsonObject.addProperty("Name", name);
        jsonObject.addProperty("Type", type());
        jsonObject.addProperty("State", state);

        JsonArray clusters = new JsonArray();
        clusterMap.forEach((hostname, cluster) -> clusters.add(cluster.toJson()));
        jsonObject.add("Clusters", clusters);

        return jsonObject;
    }
}
