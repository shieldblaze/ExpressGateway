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
package com.shieldblaze.expressgateway.core.loadbalancer;

import com.github.f4b6a3.tsid.TsidCreator;
import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.common.annotation.NonNull;
import com.shieldblaze.expressgateway.concurrent.eventstream.EventStream;
import com.shieldblaze.expressgateway.configuration.ConfigurationContext;
import com.shieldblaze.expressgateway.core.L4FrontListener;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerShutdownTask;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStartupTask;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStopTask;
import com.shieldblaze.expressgateway.core.exceptions.NotFoundException;
import com.shieldblaze.expressgateway.core.factory.EventLoopFactory;
import com.shieldblaze.expressgateway.core.factory.PooledByteBufAllocatorFactory;
import com.shieldblaze.expressgateway.core.handlers.ConnectionTracker;
import io.netty.buffer.ByteBufAllocator;
import io.netty.channel.ChannelHandler;
import lombok.extern.log4j.Log4j2;

import java.net.InetSocketAddress;
import java.util.Collections;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.atomic.AtomicInteger;

import static com.shieldblaze.expressgateway.common.utils.LogSanitizer.sanitize;
import static java.util.Objects.requireNonNull;

/**
 * {@link L4LoadBalancer} holds base functions for a L4-Load Balancer.
 */
@Log4j2
public abstract class L4LoadBalancer {

    public static final String DEFAULT = "DEFAULT";

    private static final AtomicInteger COUNTER = new AtomicInteger(0);

    private final String loadBalancerId = TsidCreator.getTsid().toString();
    private final ConnectionTracker connectionTracker = new ConnectionTracker();

    private String name = "L4LoadBalancer#" + COUNTER.incrementAndGet();

    private final EventStream eventStream;
    private final InetSocketAddress bindAddress;
    private final L4FrontListener l4FrontListener;
    private final Map<String, Cluster> clusterMap = new ConcurrentHashMap<>();
    private final ConfigurationContext configurationContext;
    private final ChannelHandler channelHandler;

    private final ByteBufAllocator byteBufAllocator;
    private final EventLoopFactory eventLoopFactory;

    private L4FrontListenerStartupTask l4FrontListenerStartupEvent;

    /**
     * @param name                 Name of this Load Balancer
     * @param bindAddress          {@link InetSocketAddress} on which {@link L4FrontListener} will bind and listen.
     * @param l4FrontListener      {@link L4FrontListener} for listening traffic
     * @param configurationContext {@link ConfigurationContext} to be applied
     * @param channelHandler       {@link ChannelHandler} to use for handling traffic
     * @throws NullPointerException If a required parameter if {@code null}
     */
    protected L4LoadBalancer(String name,
                             @NonNull InetSocketAddress bindAddress,
                             @NonNull L4FrontListener l4FrontListener,
                             @NonNull ConfigurationContext configurationContext,
                             ChannelHandler channelHandler) {

        if (name != null && !name.isEmpty()) {
            this.name = name;
        }

        this.bindAddress = bindAddress;
        this.l4FrontListener = l4FrontListener;
        this.configurationContext = configurationContext;
        eventStream = configurationContext.eventStreamConfiguration().newEventStream();
        this.channelHandler = channelHandler;

        byteBufAllocator = new PooledByteBufAllocatorFactory(configurationContext.bufferConfiguration()).instance();
        eventLoopFactory = new EventLoopFactory(configurationContext);

        l4FrontListener.l4LoadBalancer(this);
    }

    /**
     * Load Balancer ID
     *
     * @return Returns the Load Balancer ID
     */
    public String id() {
        return loadBalancerId;
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
    public L4FrontListenerStartupTask start() {
        try {
            log.info("Trying to start L4FrontListener");

            // Start the listener
            l4FrontListenerStartupEvent = l4FrontListener.start();
            // LOW-27: Log success only in the success path, not in finally block
            log.info("Started L4FrontListener: {}", l4FrontListenerStartupEvent);
            return l4FrontListenerStartupEvent;
        } catch (Exception ex) {
            log.fatal("Failed to start L4FrontListener", ex);
            throw ex;
        }
    }

    /**
     * Stop L4 Load Balancer, and it's child operations and services.
     *
     * @return {@link L4FrontListenerStopTask} instance
     */
    public L4FrontListenerStopTask stop() {
        try {
            log.info("Trying to stop L4FrontListener");

            // Stop the listener
            L4FrontListenerStopTask event = l4FrontListener.stop();
            // LOW-27: Log success only in the success path, not in finally block
            log.info("Stopped L4FrontListener: {}", event);
            return event;
        } catch (Exception ex) {
            log.fatal("Failed to stop L4FrontListener", ex);
            throw ex;
        }
    }

    /**
     * Shutdown L4 Load Balancer
     *
     * @return {@link L4FrontListenerShutdownTask} instance
     */
    public L4FrontListenerShutdownTask shutdown() {
        try {
            log.info("Trying to shutdown L4FrontListener");

            // Shutdown the listener
            L4FrontListenerShutdownTask event = l4FrontListener.shutdown();

            // Release event loop threads -- without this, the JVM will not exit cleanly
            // because the non-daemon EventLoop threads keep running after the listener stops.
            eventLoopFactory.shutdown();

            // LOW-27: Log success only in the success path, not in finally block
            log.info("Shutdown L4FrontListener: {}", event);
            return event;
        } catch (Exception ex) {
            log.fatal("Failed to shutdown L4FrontListener", ex);
            throw ex;
        }
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
     * @param hostname FQDN to lookup for
     * @throws NullPointerException If {@link Cluster} is not found
     */
    @NonNull
    public Cluster cluster(String hostname) {
        // Sanitize hostname for logging to prevent log injection (CWE-117).
        String safeHostname = sanitize(hostname);
        log.debug("Looking up for Cluster with hostname: {}", safeHostname);
        try {
            Cluster cluster = clusterMap.get(hostname);

            // If Cluster is not found, then lookup for DEFAULT Cluster
            if (cluster == null) {

                // If DEFAULT Cluster is not found, then throw exception
                cluster = clusterMap.get("DEFAULT");
                if (cluster == null) {
                    throw new NotFoundException("Cluster not found with Hostname: " + safeHostname);
                }
            }
            return cluster;
        } catch (Exception ex) {
            log.error("Failed to lookup for Cluster", ex);
            throw ex;
        }
    }

    /**
     * Get all {@link Cluster} instances linked with this Load Balancer
     *
     * @return Unmodifiable Map of {@link Cluster} instances
     */
    public Map<String, Cluster> clusters() {
        return Collections.unmodifiableMap(clusterMap);
    }

    /**
     * Remove all Clusters from Load Balancer
     */
    public void removeClusters() {
        clusterMap.clear();
    }

    /**
     * Set the default {@link Cluster}
     */
    public void defaultCluster(Cluster cluster) {
        mappedCluster("DEFAULT", cluster);
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
    public void mappedCluster(String hostname, Cluster cluster) {
        requireNonNull(hostname, "Hostname");
        requireNonNull(cluster, "Cluster");

        try {
            log.info("Mapping Cluster: {} with Hostname: {} and EventStream: {}", cluster, hostname, eventStream);

            cluster.useEventStream(eventStream);
            clusterMap.put(hostname, cluster);

            log.info("Successfully mapped Cluster");
        } catch (Exception ex) {
            log.error("Failed to map cluster", ex);
            throw ex;
        }
    }

    /**
     * Remap a {@link Cluster} from old hostname to new hostname.
     *
     * @param oldHostname Old Hostname
     * @param newHostname New Hostname
     *
     * @return Returns {@code true} if remapping was successful else {@code false}
     */
    public boolean remapCluster(String oldHostname, String newHostname) {
        requireNonNull(oldHostname, "OldHostname");
        requireNonNull(newHostname, "NewHostname");

        try {
            log.info("Remapping Cluster from Hostname: {} to Hostname: {}", oldHostname, newHostname);

            // Atomic compute to avoid TOCTOU between remove(old) and put(new).
            // If two threads remap concurrently, the non-atomic remove+put could
            // leave the cluster missing from both keys.
            Cluster[] holder = new Cluster[1];
            clusterMap.compute(oldHostname, (key, existing) -> {
                holder[0] = existing;
                return null; // remove old mapping
            });

            if (holder[0] == null) {
                return false;
            }

            clusterMap.put(newHostname, holder[0]);
            log.info("Successfully remapped Cluster: {}, from Hostname: {} to Hostname: {}", holder[0], oldHostname, newHostname);
            return true;
        } catch (Exception ex) {
            log.error("Failed to Remap Cluster, oldHostname {}, newHostname {}", oldHostname, newHostname, ex);
            throw ex;
        }
    }

    /**
     * Remove a Cluster from mapping
     *
     * @param hostname Hostname of the Cluster
     * @return Returns {@link Boolean#TRUE} if removal was successful else {@link Boolean#FALSE}
     */
    public boolean removeClusters(String hostname) {
        try {
            Cluster cluster = clusterMap.remove(hostname);
            if (cluster == null) {
                log.info("No Cluster found for Hostname mapping: {}", hostname);
                return false;
            }

            cluster.close();
            log.info("Successfully removed Cluster from Hostname mapping: {}", hostname);
            return true;
        } catch (Exception ex) {
            log.error("Failed to remove Hostname: {} from Cluster", hostname, ex);
            throw ex;
        }
    }

    /**
     * Get {@link ConfigurationContext} which is applied
     */
    public ConfigurationContext configurationContext() {
        return configurationContext;
    }

    /**
     * Get {@link ChannelHandler} used for handling traffic
     */
    public ChannelHandler channelHandler() {
        return channelHandler;
    }

    /**
     * Get {@link ByteBufAllocator} created from {@link PooledByteBufAllocatorFactory}
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

    @Override
    public String toString() {
        return "L4LoadBalancer{" + toJson() + '}';
    }

    /**
     * Get the {@link L4FrontListenerStartupTask} instance
     */
    public L4FrontListenerStartupTask event() {
        return l4FrontListenerStartupEvent;
    }

    /**
     * Convert Load Balancer data into {@link JsonObject}
     *
     * @return {@link JsonObject} Instance
     */
    public JsonObject toJson() {
        JsonObject jsonObject = new JsonObject();

        String state;
        if (l4FrontListenerStartupEvent == null) {
            state = "Not Started";
        } else if (l4FrontListenerStartupEvent.isFinished()) {
            if (l4FrontListenerStartupEvent.isSuccess()) {
                state = "Running";
            } else {
                state = "Failed; " + l4FrontListenerStartupEvent.taskError();
            }
        } else {
            state = "Pending";
        }

        jsonObject.addProperty("ID", loadBalancerId);
        jsonObject.addProperty("Name", name);
        jsonObject.addProperty("Type", type());
        jsonObject.addProperty("State", state);

        JsonArray clusters = new JsonArray();
        clusterMap.forEach((hostname, cluster) -> clusters.add(cluster.toJson()));
        jsonObject.add("Clusters", clusters);

        return jsonObject;
    }
}
