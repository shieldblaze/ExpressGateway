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

import java.io.IOException;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.net.URI;
import java.net.UnknownHostException;
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
    private HealthCheckTemplate template;

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
    public boolean addNode(Node node) throws UnknownHostException {
        for (Node n : nodes) {
            // If both Nodes are same then don't add and return false.
            if (node.equals(n)) {
                return false;
            }
        }

        configureHealthCheckForNode(node); // Add HealthCheck if required
        nodes.add(node);                   // Add this Node into the list
        eventStream.publish(new NodeAddedEvent(node)); // Publish NodeAddedEvent event
        return true;
    }

    /**
     * Remove {@link Node} from this {@linkplain Cluster}
     *
     * @param node {@link Node} to be removed
     * @return {@link Boolean#TRUE} if removal was successful else {@link Boolean#FALSE}
     */
    @InternalCall
    @NonNull
    public boolean removeNode(Node node) {
        boolean isFound = false;
        for (Node n : nodes) {
            if (node.equals(n)) {
                isFound = true;
                break;
            }
        }

        if (!isFound) {
            return false;
        }

        if (healthCheckService != null) {
            healthCheckService.remove(node);
        }

        nodes.remove(node); // Remove the Node from the list
        eventStream.publish(new NodeRemovedEvent(node)); // Publish NodeRemovedEvent event
        return true;
    }

    /**
     * Get a {@link Node} using it's ID
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
     * Get List of all {@link Node} associated with this {@linkplain Cluster}
     */
    public List<Node> nodes() {
        return ONLINE_NODES;
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
     * Set the {@link EventStream} to use.
     * This method will be called by L4LoadBalancer when this
     * Cluster is being mapped.
     *
     * @param eventStream {@link EventStream}
     * @throws IllegalStateException When Load Balancer is already set
     */
    @InternalCall(index = 1)
    @NonNull
    public void useMainEventStream(EventStream eventStream) {
        EventStream oldEventStream = this.eventStream;

        eventStream.addSubscribersFrom(this.eventStream);
        this.eventStream = eventStream;

        // Close the old EventStream
        oldEventStream.close();
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
            // If LoadBalance has changed then unsubscribe it
            // from the old EventStream and close the LoadBalance.
            if (this.loadBalance != null) {
                this.eventStream.unsubscribe(loadBalance);
                this.loadBalance.close();
            }
        } catch (IOException e) {
            Error error = new Error(e);
            logger.error("Error while closing LoadBalance", error);
            throw error;
        }

        loadBalance.cluster(this);
        this.loadBalance = loadBalance;
        this.eventStream.subscribe(loadBalance); // Subscribe this LoadBalance to the EventStream
    }

    public EventStream eventStream() {
        return eventStream;
    }

    /**
     * Returns the {@link HealthCheckTemplate}
     */
    public HealthCheckTemplate healthCheckTemplate() {
        return template;
    }

    @NonNull
    void configureHealthCheck(HealthCheckConfiguration healthCheckConfiguration, HealthCheckTemplate healthCheckTemplate) {
        this.healthCheckService = new HealthCheckService(healthCheckConfiguration, eventStream);
        this.template = healthCheckTemplate;
    }

    @NonNull
    private void configureHealthCheckForNode(Node node) throws UnknownHostException {
        if (healthCheckService == null) {
            return;
        }

        HealthCheck healthCheck;
        switch (template.protocol()) {
            case TCP:
                healthCheck = new TCPHealthCheck(new InetSocketAddress(template.host(), template.port()), Duration.ofSeconds(template.timeout()), template.samples());
                break;
            case UDP:
                healthCheck = new UDPHealthCheck(new InetSocketAddress(template.host(), template.port()), Duration.ofSeconds(template.timeout()), template.samples());
                break;
            case HTTP:
            case HTTPS:
                String host;
                if (template.protocol() == HealthCheckTemplate.Protocol.HTTP) {
                    host = "http://" + InetAddress.getByName(template.host()).getHostAddress() + ":" + healthCheckTemplate().port();
                } else {
                    host = "https://" + InetAddress.getByName(template.host()).getHostAddress() + ":" + healthCheckTemplate().port();
                }

                healthCheck = new HTTPHealthCheck(URI.create(host), Duration.ofSeconds(template.timeout()), template.samples());
                break;
            default:
                Error error = new Error("Unknown HealthCheck Protocol: " + template.protocol());
                logger.fatal(error);
                throw error;
        }

        // Associate HealthCheck with Node
        node.healthCheck(healthCheck);
        healthCheckService.add(node);
    }

    /**
     * Shutdown the entire Cluster including all {@link Node} and {@link Connection}.
     * <p>
     * This method is called by L4LoadBalancer when this Cluster is being unmapped.
     */
    @InternalCall(index = 3)
    public void close() {
        nodes.forEach(node -> {
            try {
                node.close();
                eventStream.publish(new NodeRemovedEvent(node));
            } catch (Exception ex) {
                // Ignore
            }
        });
        nodes.clear();
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
