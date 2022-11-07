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
package com.shieldblaze.expressgateway.backend.cluster;

import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import com.shieldblaze.expressgateway.backend.Connection;
import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.events.node.NodeAddedEvent;
import com.shieldblaze.expressgateway.backend.events.node.NodeRemovedEvent;
import com.shieldblaze.expressgateway.backend.exceptions.LoadBalanceException;
import com.shieldblaze.expressgateway.backend.healthcheck.HealthCheckService;
import com.shieldblaze.expressgateway.backend.healthcheck.HealthCheckTemplate;
import com.shieldblaze.expressgateway.backend.loadbalance.LoadBalance;
import com.shieldblaze.expressgateway.backend.loadbalance.Request;
import com.shieldblaze.expressgateway.backend.loadbalance.Response;
import com.shieldblaze.expressgateway.common.annotation.InternalCall;
import com.shieldblaze.expressgateway.common.annotation.NonNull;
import com.shieldblaze.expressgateway.concurrent.eventstream.EventStream;
import com.shieldblaze.expressgateway.configuration.healthcheck.HealthCheckConfiguration;
import com.shieldblaze.expressgateway.healthcheck.HealthCheck;
import com.shieldblaze.expressgateway.healthcheck.l4.TCPHealthCheck;
import com.shieldblaze.expressgateway.healthcheck.l4.UDPHealthCheck;
import com.shieldblaze.expressgateway.healthcheck.l7.HTTPHealthCheck;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.net.URI;
import java.time.Duration;
import java.util.List;
import java.util.concurrent.CopyOnWriteArrayList;

/**
 * <p> A {@linkplain Cluster} is a collection of servers. It is also responsible for
 * load-balancing, addition and removal of servers. </p>
 *
 * <ul>
 *
 * <li> {@linkplain Cluster} uses {@link EventStream} to publish {@link NodeAddedEvent} and
 * {@link NodeRemovedEvent} events on addition and removal of server {@link Node}.</li>
 *
 * <li> {@linkplain Cluster} uses {@linkplain LoadBalance} to load-balance when called
 * {@link #nextNode(Request)}. </li>
 *
 * <li> {@linkplain Cluster} uses {@link HealthCheckTemplate} to associate {@link HealthCheck}
 * with server ({@link Node}). </li>
 *
 * </ul>
 *
 * <p> Implementation: {@linkplain Cluster} should be initialized and attached to the load-Balancer after
 * it is successfully built and started. Once attached, {@linkplain Node} can be added and all other
 * functionality can be performed seamlessly. </p>
 */
public class Cluster extends ClusterOnlineNodesWorker {

    private static final Logger logger = LogManager.getLogger(Cluster.class);

    private final List<Node> nodes = new CopyOnWriteArrayList<>();
    private EventStream eventStream = new EventStream();
    private LoadBalance<?, ?, ?, ?> loadBalance;
    private HealthCheckService healthCheckService;
    private HealthCheckTemplate healthCheckTemplate;

    Cluster(LoadBalance<?, ?, ?, ?> loadBalance) {
        loadBalance(loadBalance);
        this.eventStream.subscribe(this);
    }

    /**
     * Add {@link Node} into this {@linkplain Cluster}
     *
     * @return {@link Boolean#TRUE} if addition was successful else {@link Boolean#FALSE}
     */
    @InternalCall
    @NonNull
    public boolean addNode(Node node) throws Exception {
        try {
            logger.info("Adding Node: {} into Cluster: {}", node, this);

            for (Node n : nodes) {
                // If Node already exists then don't add
                if (node.equals(n)) {
                    logger.info("Duplicate node detected: {}", n);
                    return false;
                }
            }

            configureHealthCheckForNode(node); // Add HealthCheck if required
            nodes.add(node);                   // Add this Node into the list
            eventStream.publish(new NodeAddedEvent(node)); // Publish NodeAddedEvent event

            logger.info("Successfully added Node: {} into Cluster: {}", node, this);
            return true;
        } catch (Exception ex) {
            logger.error("Failed to add Node: {} into Cluster: {}", node, this);
            throw ex;
        }
    }

    /**
     * Remove {@link Node} from this {@linkplain Cluster}
     *
     * @param node {@link Node} to be removed
     * @return {@link Boolean#TRUE} if removal was successful else {@link Boolean#FALSE}
     * @throws NullPointerException If node was not found in this Cluster
     */
    @InternalCall
    @NonNull
    public boolean removeNode(Node node) {
        try {
            logger.info("Removing Node: {} from Cluster: {}", node, this);

            // If Node could not be removed then it was not found
            if (!nodes.remove(node)) {
                throw new NullPointerException("Node not found in Cluster");
            }

            // If Health check service is available then remove the node from that too.
            if (healthCheckService != null) {
                healthCheckService.remove(node);
            }

            eventStream.publish(new NodeRemovedEvent(node)); // Publish NodeRemovedEvent event

            logger.info("Successfully removed Node: {} from Cluster: {}", node, this);
            return true;
        } catch (Exception ex) {
            logger.error("Failed to remove Node:{} from Cluster: {}", node, this);
            throw ex;
        }
    }

    /**
     * Get a {@link Node} using its ID
     *
     * @param id {@link Node} ID
     * @return {@link Node} Instance
     * @throws NullPointerException If {@link Node} is not found
     */
    public Node get(String id) {
        for (Node node : nodes) {
            if (node.id().equalsIgnoreCase(id)) {
                return node;
            }
        }

        throw new NullPointerException("Node not found with ID: " + id);
    }

    /**
     * Get List of online {@link Node} associated with this {@linkplain Cluster}
     */
    public List<Node> onlineNodes() {
        return ONLINE_NODES;
    }

    /**
     * Get List of all {@link Node} associated with this {@linkplain Cluster}
     */
    public List<Node> allNodes() {
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

    /**
     * <p>
     * Set the {@link EventStream} to use.
     * This method will be called by L4LoadBalancer when this
     * Cluster is being mapped.
     * </p>
     *
     * <p>
     * <h3> NOTE: THIS METHOD DOES NOT CLOSE OLD EVENTSTREAM. SO CLOSE OLD EVENTSTREAM WHEN DONE WITH IT. </h3>
     * </p>
     *
     * @param eventStream New {@link EventStream} instance
     * @throws IllegalStateException When Load Balancer is already set
     */
    @InternalCall(index = 1)
    @NonNull
    public void useEventStream(EventStream eventStream) {
        try {
            logger.info("Configuring Cluster: {} using Old EventStream: {} to use New EventStream: {}",
                    this, this.eventStream, eventStream);

            eventStream.addSubscribersFrom(this.eventStream);
            this.eventStream = eventStream;

            logger.info("Successfully configured Cluster: {} to use new EventStream: {}", this, eventStream);
        } catch (Exception ex) {
            logger.error("Failed to configure cluster to use new EventStream", ex);
            throw ex;
        }
    }

    /**
     * <p> Set a new {@link LoadBalance} for load-balancing. </p>
     * If old {@link LoadBalance} is present then unsubscribe it from the
     * {@link EventStream} and close it using {@link LoadBalance#close()}.
     *
     * @param loadBalance New {@link LoadBalance} implementation to use for load-balancing
     */
    @NonNull
    public void loadBalance(LoadBalance<?, ?, ?, ?> loadBalance) {
        try {
            logger.info("Configuration Cluster: {} to use LoadBalance: {}", this, loadBalance);

            // If LoadBalance has changed then unsubscribe it from the old EventStream
            // because we will be firing events to new LoadBalance
            if (this.loadBalance != null) {
                eventStream.unsubscribe(loadBalance);
            }

            eventStream.subscribe(loadBalance); // Subscribe this LoadBalance to the EventStream
            loadBalance.cluster(this);
            this.loadBalance = loadBalance;

            logger.info("Successfully configured Cluster: {} to use new LoadBalance: {}", this, loadBalance);
        } catch (Exception ex) {
            logger.error("Failed to configure cluster to use new LoadBalance", ex);
            throw ex;
        }
    }

    public EventStream eventStream() {
        return eventStream;
    }

    /**
     * Returns the {@link HealthCheckTemplate}
     */
    public HealthCheckTemplate healthCheckTemplate() {
        return healthCheckTemplate;
    }

    @NonNull
    void configureHealthCheck(HealthCheckConfiguration healthCheckConfiguration, HealthCheckTemplate healthCheckTemplate) {
        this.healthCheckService = new HealthCheckService(healthCheckConfiguration, eventStream);
        this.healthCheckTemplate = healthCheckTemplate;
    }

    @NonNull
    private void configureHealthCheckForNode(Node node) throws Exception {
        try {
            logger.info("Configuring Node: {} in Cluster: {} to use HealthCheckService", node, this);

            if (healthCheckTemplate() == null) {
                logger.info("HealthCheckService is not enabled");
                return;
            }

            HealthCheck healthCheck;
            switch (healthCheckTemplate().protocol()) {
                case TCP ->
                        healthCheck = new TCPHealthCheck(new InetSocketAddress(healthCheckTemplate.host(), healthCheckTemplate.port()), Duration.ofSeconds(healthCheckTemplate.timeout()), healthCheckTemplate.samples());
                case UDP ->
                        healthCheck = new UDPHealthCheck(new InetSocketAddress(healthCheckTemplate.host(), healthCheckTemplate.port()), Duration.ofSeconds(healthCheckTemplate.timeout()), healthCheckTemplate.samples());
                case HTTP, HTTPS -> {
                    String host;
                    if (healthCheckTemplate.protocol() == HealthCheckTemplate.Protocol.HTTP) {
                        host = "http://" + InetAddress.getByName(healthCheckTemplate.host()).getHostAddress() + ":" + healthCheckTemplate().port();
                    } else {
                        host = "https://" + InetAddress.getByName(healthCheckTemplate.host()).getHostAddress() + ":" + healthCheckTemplate().port();
                    }
                    healthCheck = new HTTPHealthCheck(URI.create(host), Duration.ofSeconds(healthCheckTemplate.timeout()), healthCheckTemplate.samples());
                }
                default -> {
                    Error error = new Error("Unknown HealthCheck Protocol: " + healthCheckTemplate.protocol());
                    logger.fatal("Failed to select HealthCheck protocol: " + healthCheckTemplate.protocol());
                    throw error;
                }
            }
            logger.info("Selected HealthCheck: {} for Node: {}", healthCheck, node);

            // Associate HealthCheck with Node
            node.healthCheck(healthCheck);
            healthCheckService.add(node);

            logger.info("Successfully configured HealthCheck for Node: {} in Cluster: {}", node, this);
        } catch (Exception ex) {
            logger.fatal("Failed to configure Node to use HealthCheckService", ex);
            throw ex;
        }
    }

    /**
     * Shutdown the entire Cluster including all {@link Node} and {@link Connection}.
     * <p>
     * This method is called by L4LoadBalancer when this Cluster is being unmapped.
     */
    @InternalCall(index = 3)
    public void close() {
        try {
            logger.info("Shutting down Cluster: {} and removing all Nodes: {}", this, nodes);

            nodes.forEach(node -> {
                try {
                    // Just close the node because node will take
                    // care of removing itself from this Cluster.
                    node.close();
                    eventStream.publish(new NodeRemovedEvent(node));

                    logger.info("Closed Node: {} from Cluster: {}", node, this);
                } catch (Exception ex) {
                    logger.error("Failed to close Node: {} from Cluster: {}", node, this);
                }
            });
            nodes.clear();

            logger.info("Successfully shutdown Cluster: {}", this);
        } catch (Exception ex) {
            logger.error("Failed to shutdown Cluster", ex);
            throw ex;
        }
    }

    @Override
    public String toString() {
        return "Cluster{" +
                "nodes=" + nodes.size() +
                ", eventStream=" + eventStream +
                ", loadBalance=" + loadBalance.name() +
                ", healthCheckService=" + healthCheckService +
                ", healthCheckTemplate=" + healthCheckTemplate +
                '}';
    }

    /**
     * Convert Cluster data into {@link JsonObject}
     *
     * @return {@link JsonObject} Instance
     */
    public JsonObject toJson() {
        JsonObject jsonObject = new JsonObject();

        JsonArray nodesArray = new JsonArray();
        for (Node node : nodes) {
            nodesArray.add(node.toJson());
        }

        jsonObject.add("Nodes", nodesArray);
        return jsonObject;
    }
}
