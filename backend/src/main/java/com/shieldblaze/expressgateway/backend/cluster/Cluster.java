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
package com.shieldblaze.expressgateway.backend.cluster;

import com.shieldblaze.expressgateway.backend.Connection;
import com.shieldblaze.expressgateway.backend.HealthCheckTemplate;
import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.events.node.NodeAddedEvent;
import com.shieldblaze.expressgateway.backend.events.node.NodeRemovedEvent;
import com.shieldblaze.expressgateway.backend.exceptions.LoadBalanceException;
import com.shieldblaze.expressgateway.backend.healthcheck.HealthCheckService;
import com.shieldblaze.expressgateway.backend.loadbalance.LoadBalance;
import com.shieldblaze.expressgateway.backend.loadbalance.Request;
import com.shieldblaze.expressgateway.backend.loadbalance.Response;
import com.shieldblaze.expressgateway.common.annotation.InternalCall;
import com.shieldblaze.expressgateway.common.annotation.NonNull;
import com.shieldblaze.expressgateway.concurrent.eventstream.EventStream;
import com.shieldblaze.expressgateway.configuration.CoreConfiguration;
import com.shieldblaze.expressgateway.configuration.healthcheck.HealthCheckConfiguration;
import com.shieldblaze.expressgateway.healthcheck.HealthCheck;
import com.shieldblaze.expressgateway.healthcheck.l4.TCPHealthCheck;
import com.shieldblaze.expressgateway.healthcheck.l4.UDPHealthCheck;
import com.shieldblaze.expressgateway.healthcheck.l7.HTTPHealthCheck;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.io.IOException;
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
public abstract class Cluster {

    private static final Logger logger = LogManager.getLogger(Cluster.class);

    private final List<Node> nodes = new CopyOnWriteArrayList<>();

    private EventStream eventStream;
    private LoadBalance<?, ?, ?, ?> loadBalance;
    private HealthCheckTemplate healthCheckTemplate;
    private HealthCheckConfiguration healthCheckConfiguration;
    private HealthCheckService healthCheckService;

    /**
     * Create a new {@link Cluster} Instance with specified {@link LoadBalance}
     * and without {@link HealthCheckTemplate} (Health Check for all servers in
     * this {@link Cluster} is disabled).
     *
     * @param loadBalance {@link LoadBalance} implementation to use for load-balancing
     */
    @NonNull
    public Cluster(LoadBalance<?, ?, ?, ?> loadBalance) {
        this(loadBalance, null);
    }

    /**
     * Create a new {@link Cluster} Instance with specified {@link LoadBalance}
     * and {@link HealthCheckTemplate}.
     *
     * @param loadBalance         {@link LoadBalance} implementation to use for load-balancing
     * @param healthCheckTemplate {@link HealthCheckTemplate} to create {@link HealthCheck}
     *                            for all servers {@link Node}.
     */
    @NonNull
    public Cluster(LoadBalance<?, ?, ?, ?> loadBalance, HealthCheckTemplate healthCheckTemplate) {
        loadBalance(loadBalance);
        this.healthCheckTemplate = healthCheckTemplate;
    }

    /**
     * Add {@link Node} into this {@linkplain Cluster}
     *
     * @return {@link Boolean#TRUE} if addition was successful else {@link Boolean#FALSE}
     */
    @NonNull
    public boolean addNode(Node node) {
        for (Node n : nodes) {
            // If both SocketAddress are same then don't add and return false.
            if (n.socketAddress() == node.socketAddress()) {
                return false;
            }
        }

        healthCheck(node); // Add HealthCheck if required
        nodes.add(node);   // Add this Node into the list
        eventStream.publish(new NodeAddedEvent(node)); // Publish NodeAddedEvent event
        return true;
    }

    /**
     * Remove {@link Node} from this {@linkplain Cluster}
     *
     * @param node {@link Node} to be removed
     * @return {@link Boolean#TRUE} if removal was successful else {@link Boolean#FALSE}
     */
    @NonNull
    public boolean removeNode(Node node) {
        boolean isFound = false;
        for (Node n : nodes) {
            // If both SocketAddress are same then break the loop and remove the node
            if (n.socketAddress() == node.socketAddress()) {
                isFound = true;
                break;
            }
        }

        if (!isFound) {
            return false;
        }

        node.close();       // Close the Node
        nodes.remove(node); // Remove the Node from the list
        eventStream.publish(new NodeRemovedEvent(node)); // Publish NodeRemovedEvent event
        return true;
    }

    /**
     * Get List of all {@link Node} associated with this {@linkplain Cluster}
     */
    public List<Node> nodes() {
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
     * Set the {@link EventStream} to use.
     * This method will be called by L4LoadBalancer when this
     * Cluster is being mapped.
     *
     * @param eventStream {@link EventStream}
     * @throws IllegalStateException When Load Balancer is already set
     */
    @InternalCall
    public void eventStream(EventStream eventStream) {
        if (this.eventStream != null) {
            throw new IllegalStateException("EventStream is already set");
        }
        this.eventStream = eventStream;
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
            if (loadBalance != null) {
                eventStream.unsubscribe(loadBalance);
                this.loadBalance.close();
            }
        } catch (IOException e) {
            logger.error("Error while closing LoadBalance", e);
        }

        this.loadBalance = loadBalance;     // Set the new Load Balance
        eventStream.subscribe(loadBalance); // Subscribe to the EventStream
    }

    /**
     * Returns the {@link HealthCheckTemplate}
     */
    public HealthCheckTemplate healthCheckTemplate() {
        return healthCheckTemplate;
    }

    /**
     * Set the {@link HealthCheckTemplate}
     */
    @NonNull
    public void healthCheckTemplate(HealthCheckTemplate healthCheckTemplate) {
        this.healthCheckTemplate = healthCheckTemplate;
    }

    @NonNull
    @InternalCall
    public void healthCheckConfiguration(HealthCheckConfiguration healthCheckConfiguration) {
        this.healthCheckConfiguration = healthCheckConfiguration;
    }

    @NonNull
    private void healthCheck(Node node) {
        if (healthCheckTemplate == null) {
            return;
        }

        HealthCheck healthCheck = null;
        if (healthCheckTemplate.protocol() == HealthCheckTemplate.Protocol.TCP) {
            healthCheck = new TCPHealthCheck(new InetSocketAddress(node.socketAddress().getAddress(), healthCheckTemplate.port()),
                    Duration.ofSeconds(healthCheckTemplate().timeout()), healthCheckTemplate().samples());
        } else if (healthCheckTemplate.protocol() == HealthCheckTemplate.Protocol.UDP) {
            healthCheck = new UDPHealthCheck(new InetSocketAddress(node.socketAddress().getAddress(), healthCheckTemplate.port()),
                    Duration.ofSeconds(healthCheckTemplate().timeout()), healthCheckTemplate().samples());
        } else if (healthCheckTemplate.protocol() == HealthCheckTemplate.Protocol.HTTP ||
                healthCheckTemplate.protocol() == HealthCheckTemplate.Protocol.HTTPS) {

            String host;
            if (healthCheckTemplate.protocol() == HealthCheckTemplate.Protocol.HTTP) {
                host = "http://" + node.socketAddress().getAddress().getHostAddress() + ":" + healthCheckTemplate().port();
            } else {
                host = "https://" + node.socketAddress().getAddress().getHostAddress() + ":" + healthCheckTemplate().port();
            }

            healthCheck = new HTTPHealthCheck(URI.create(host), Duration.ofSeconds(healthCheckTemplate().timeout()), healthCheckTemplate().samples());
        }

        if (healthCheck != null) {
            node.healthCheck(healthCheck);
        }
    }

    /**
     * Shutdown the entire Cluster including all {@link Node} and {@link Connection}.
     *
     * This method is called by L4LoadBalancer when this Cluster is being unmapped.
     */
    @InternalCall
    public void close() {
        for (Node node : nodes) {
            try {
                node.close();
                eventStream.publish(new NodeRemovedEvent(node));
            } catch (Exception ex) {
                // Ignore
            }
        }

        nodes.clear();
    }
}
