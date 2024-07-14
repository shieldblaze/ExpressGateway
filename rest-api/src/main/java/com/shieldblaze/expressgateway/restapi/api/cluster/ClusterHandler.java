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
package com.shieldblaze.expressgateway.restapi.api.cluster;

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterBuilder;
import com.shieldblaze.expressgateway.backend.loadbalance.LoadBalance;
import com.shieldblaze.expressgateway.backend.loadbalance.SessionPersistence;
import com.shieldblaze.expressgateway.backend.strategy.l4.LeastConnection;
import com.shieldblaze.expressgateway.backend.strategy.l4.LeastLoad;
import com.shieldblaze.expressgateway.backend.strategy.l4.Random;
import com.shieldblaze.expressgateway.backend.strategy.l4.RoundRobin;
import com.shieldblaze.expressgateway.backend.strategy.l4.sessionpersistence.FourTupleHash;
import com.shieldblaze.expressgateway.backend.strategy.l4.sessionpersistence.NOOPSessionPersistence;
import com.shieldblaze.expressgateway.backend.strategy.l4.sessionpersistence.SourceIPHash;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.HTTPBalanceRequest;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.HTTPBalanceResponse;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.HTTPRandom;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.HTTPRoundRobin;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.sessionpersistence.StickySession;
import com.shieldblaze.expressgateway.configuration.healthcheck.HealthCheckConfiguration;
import com.shieldblaze.expressgateway.core.cluster.CoreContext;
import com.shieldblaze.expressgateway.core.cluster.LoadBalancerContext;
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancer;
import com.shieldblaze.expressgateway.restapi.response.FastBuilder;
import com.shieldblaze.expressgateway.restapi.response.builder.APIResponse;
import com.shieldblaze.expressgateway.restapi.response.builder.Result;
import io.netty.handler.codec.http.HttpResponseStatus;
import org.springframework.http.MediaType;
import org.springframework.http.ResponseEntity;
import org.springframework.web.bind.annotation.DeleteMapping;
import org.springframework.web.bind.annotation.PostMapping;
import org.springframework.web.bind.annotation.PutMapping;
import org.springframework.web.bind.annotation.RequestBody;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.RequestParam;
import org.springframework.web.bind.annotation.RestController;

import java.net.InetSocketAddress;
import java.util.Objects;

@RestController
@RequestMapping("/v1/cluster")
public final class ClusterHandler {

    @PostMapping(value = "/create", consumes = MediaType.APPLICATION_JSON_VALUE, produces = MediaType.APPLICATION_JSON_VALUE)
    public static ResponseEntity<String> create(@RequestParam String id, @RequestBody ClusterContext clusterContext) {
        LoadBalancerContext context = CoreContext.get(id);
        L4LoadBalancer l4LoadBalancer = context.l4LoadBalancer();

        ClusterBuilder clusterBuilder = ClusterBuilder.newBuilder();
        if (clusterContext.healthCheckTemplate() != null) {
            clusterBuilder.withHealthCheck(HealthCheckConfiguration.DEFAULT, clusterContext.healthCheckTemplate());
        }

        determineLoadBalance(l4LoadBalancer, clusterBuilder, clusterContext);

        Cluster cluster = clusterBuilder.build();
        l4LoadBalancer.mapCluster(clusterContext.hostname(), cluster);

        APIResponse apiResponse = APIResponse.newBuilder()
                .isSuccess(true)
                .withResult(Result.newBuilder().withHeader("Cluster").withMessage(cluster.toJson()).build())
                .build();

        return FastBuilder.response(apiResponse.getResponse(), HttpResponseStatus.CREATED);
    }

    @PutMapping(value = "/remap", produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> remap(@RequestParam String id, @RequestParam String oldHostname, @RequestParam String newHostname) {
        LoadBalancerContext property = CoreContext.get(id);
        L4LoadBalancer l4LoadBalancer = property.l4LoadBalancer();

        l4LoadBalancer.remapCluster(oldHostname, newHostname);

        APIResponse apiResponse = APIResponse.newBuilder()
                .isSuccess(true)
                .build();

        return FastBuilder.response(apiResponse.getResponse(), HttpResponseStatus.CREATED);
    }

    @DeleteMapping(value = "/delete", produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> delete(@RequestParam String id, @RequestParam String hostname) {
        LoadBalancerContext property = CoreContext.get(id);
        Objects.requireNonNull(hostname, "Hostname");

        boolean removed = property.l4LoadBalancer().removeCluster(hostname);
        if (!removed) {
            throw new NullPointerException("Cluster not found with Hostname: " + hostname);
        }

        APIResponse apiResponse = APIResponse.newBuilder()
                .isSuccess(true)
                .build();

        return FastBuilder.response(apiResponse.getResponse(), HttpResponseStatus.OK);
    }

    /**
     * Determine the type of {@link LoadBalance} and {@link SessionPersistence}
     * from {@link ClusterContext}
     *
     * @param l4LoadBalancer {@link L4LoadBalancer} Instance
     * @param clusterBuilder {@link ClusterBuilder} Instance where {@link LoadBalance}
     *                       and {@link SessionPersistence} will be applied.
     * @param clusterContext {@link ClusterContext} Instance
     */
    private static void determineLoadBalance(L4LoadBalancer l4LoadBalancer, ClusterBuilder clusterBuilder, ClusterContext clusterContext) {
        // Determine LoadBalance and SessionPersistence for L4 and L7/HTTP
        if ("L4".equalsIgnoreCase(l4LoadBalancer.type())) {
            LoadBalance<Node, Node, InetSocketAddress, Node> loadBalance;
            SessionPersistence<Node, Node, InetSocketAddress, Node> sessionPersistence;

            if ("FiveTupleHash".equalsIgnoreCase(clusterContext.sessionPersistence())) {
                sessionPersistence = new FourTupleHash();
            } else if ("SourceIPHash".equalsIgnoreCase(clusterContext.sessionPersistence())) {
                sessionPersistence = new SourceIPHash();
            } else if ("NOOP".equalsIgnoreCase(clusterContext.sessionPersistence())) {
                sessionPersistence = NOOPSessionPersistence.INSTANCE;
            } else {
                throw new IllegalArgumentException("Invalid SessionPersistence: " + clusterContext.sessionPersistence());
            }

            if ("LeastConnection".equalsIgnoreCase(clusterContext.loadBalance())) {
                loadBalance = new LeastConnection(sessionPersistence);
            } else if ("LeastLoad".equalsIgnoreCase(clusterContext.loadBalance())) {
                loadBalance = new LeastLoad(sessionPersistence);
            } else if ("Random".equalsIgnoreCase(clusterContext.loadBalance())) {
                loadBalance = new Random(sessionPersistence);
            } else if ("RoundRobin".equalsIgnoreCase(clusterContext.loadBalance())) {
                loadBalance = new RoundRobin(sessionPersistence);
            } else {
                throw new IllegalArgumentException("Invalid LoadBalance: " + clusterContext.loadBalance());
            }

            clusterBuilder.withLoadBalance(loadBalance);
        } else if ("L7/HTTP".equalsIgnoreCase(l4LoadBalancer.type())) {
            LoadBalance<HTTPBalanceResponse, HTTPBalanceResponse, HTTPBalanceRequest, Node> loadBalance;
            SessionPersistence<HTTPBalanceResponse, HTTPBalanceResponse, HTTPBalanceRequest, Node> sessionPersistence;

            if ("StickySession".equalsIgnoreCase(clusterContext.sessionPersistence())) {
                sessionPersistence = new StickySession();
            } else if ("NOOP".equalsIgnoreCase(clusterContext.sessionPersistence())) {
                sessionPersistence = com.shieldblaze.expressgateway.backend.strategy.l7.http.sessionpersistence.NOOPSessionPersistence.INSTANCE;
            } else {
                throw new IllegalArgumentException("Invalid SessionPersistence: " + clusterContext.sessionPersistence());
            }

            if ("HTTPRandom".equalsIgnoreCase(clusterContext.loadBalance())) {
                loadBalance = new HTTPRandom(sessionPersistence);
            } else if ("HTTPRoundRobin".equalsIgnoreCase(clusterContext.loadBalance())) {
                loadBalance = new HTTPRoundRobin(sessionPersistence);
            } else {
                throw new IllegalArgumentException("Invalid LoadBalance: " + clusterContext.loadBalance());
            }

            clusterBuilder.withLoadBalance(loadBalance);
        }
    }
}
