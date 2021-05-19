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
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancer;
import com.shieldblaze.expressgateway.core.registry.LoadBalancerProperty;
import com.shieldblaze.expressgateway.core.registry.LoadBalancerRegistry;
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

import javax.print.attribute.standard.Media;
import java.net.InetSocketAddress;
import java.util.Objects;

@RestController
@RequestMapping("/v1/cluster")
public final class ClusterHandler {

    @PostMapping(value = "/create", consumes = MediaType.APPLICATION_JSON_VALUE, produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> create(@RequestParam String id, @RequestBody CreateClusterStruct createClusterStruct) {
        LoadBalancerProperty property = LoadBalancerRegistry.get(id);
        L4LoadBalancer l4LoadBalancer = property.l4LoadBalancer();

        ClusterBuilder clusterBuilder = ClusterBuilder.newBuilder();
        if (createClusterStruct.healthCheckTemplate() != null) {
            clusterBuilder.withHealthCheck(HealthCheckConfiguration.load(), createClusterStruct.healthCheckTemplate());
        }

        determineLoadBalance(l4LoadBalancer, clusterBuilder, createClusterStruct);

        Cluster cluster = clusterBuilder.build();
        l4LoadBalancer.mapCluster(createClusterStruct.hostname(), cluster);

        APIResponse apiResponse = APIResponse.newBuilder()
                .isSuccess(true)
                .withResult(Result.newBuilder().withHeader("Cluster").withMessage(cluster.toJson()).build())
                .build();

        return FastBuilder.response(apiResponse.getResponse(), HttpResponseStatus.CREATED);
    }

    @PutMapping(value = "/remap", produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> remap(@RequestParam String id, @RequestParam String oldHostname, @RequestParam String newHostname) {
        LoadBalancerProperty property = LoadBalancerRegistry.get(id);
        L4LoadBalancer l4LoadBalancer = property.l4LoadBalancer();

        l4LoadBalancer.remapCluster(oldHostname, newHostname);

        APIResponse apiResponse = APIResponse.newBuilder()
                .isSuccess(true)
                .build();

        return FastBuilder.response(apiResponse.getResponse(), HttpResponseStatus.CREATED);
    }

    @DeleteMapping(value = "/delete", produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> delete(@RequestParam String id, @RequestParam String hostname) {
        LoadBalancerProperty property = LoadBalancerRegistry.get(id);
        Objects.requireNonNull(hostname, "Hostname");

        property.l4LoadBalancer().removeCluster(hostname);

        APIResponse apiResponse = APIResponse.newBuilder()
                .isSuccess(true)
                .build();

        return FastBuilder.response(apiResponse.getResponse(), HttpResponseStatus.OK);
    }

    private void determineLoadBalance(L4LoadBalancer l4LoadBalancer, ClusterBuilder clusterBuilder, CreateClusterStruct createClusterStruct) {
        // Determine LoadBalance and SessionPersistence for L4 and L7/HTTP
        if (l4LoadBalancer.type().equalsIgnoreCase("L4")) {
            LoadBalance<Node, Node, InetSocketAddress, Node> loadBalance;
            SessionPersistence<Node, Node, InetSocketAddress, Node> sessionPersistence;

            if (createClusterStruct.sessionPersistence().equalsIgnoreCase("FiveTupleHash")) {
                sessionPersistence = new FourTupleHash();
            } else if (createClusterStruct.sessionPersistence().equalsIgnoreCase("SourceIPHash")) {
                sessionPersistence = new SourceIPHash();
            } else if (createClusterStruct.sessionPersistence().equalsIgnoreCase("NOOP")) {
                sessionPersistence = NOOPSessionPersistence.INSTANCE;
            } else {
                throw new IllegalArgumentException("Invalid SessionPersistence: " + createClusterStruct.sessionPersistence());
            }

            if (createClusterStruct.loadBalance().equalsIgnoreCase("LeastConnection")) {
                loadBalance = new LeastConnection(sessionPersistence);
            } else if (createClusterStruct.loadBalance().equalsIgnoreCase("LeastLoad")) {
                loadBalance = new LeastLoad(sessionPersistence);
            } else if (createClusterStruct.loadBalance().equalsIgnoreCase("Random")) {
                loadBalance = new Random(sessionPersistence);
            } else if (createClusterStruct.loadBalance().equalsIgnoreCase("RoundRobin")) {
                loadBalance = new RoundRobin(sessionPersistence);
            } else {
                throw new IllegalArgumentException("Invalid LoadBalance: " + createClusterStruct.loadBalance());
            }

            clusterBuilder.withLoadBalance(loadBalance);
        } else if (l4LoadBalancer.type().equalsIgnoreCase("L7/HTTP")) {
            LoadBalance<HTTPBalanceResponse, HTTPBalanceResponse, HTTPBalanceRequest, Node> loadBalance;
            SessionPersistence<HTTPBalanceResponse, HTTPBalanceResponse, HTTPBalanceRequest, Node> sessionPersistence;

            if (createClusterStruct.sessionPersistence().equalsIgnoreCase("StickySession")) {
                sessionPersistence = new StickySession();
            } else if (createClusterStruct.sessionPersistence().equalsIgnoreCase("NOOP")) {
                sessionPersistence = com.shieldblaze.expressgateway.backend.strategy.l7.http.sessionpersistence.NOOPSessionPersistence.INSTANCE;
            } else {
                throw new IllegalArgumentException("Invalid SessionPersistence: " + createClusterStruct.sessionPersistence());
            }

            if (createClusterStruct.loadBalance().equalsIgnoreCase("HTTPRandom")) {
                loadBalance = new HTTPRandom(sessionPersistence);
            } else if (createClusterStruct.loadBalance().equalsIgnoreCase("HTTPRoundRobin")) {
                loadBalance = new HTTPRoundRobin(sessionPersistence);
            } else {
                throw new IllegalArgumentException("Invalid LoadBalance: " + createClusterStruct.loadBalance());
            }

            clusterBuilder.withLoadBalance(loadBalance);
        }
    }
}
