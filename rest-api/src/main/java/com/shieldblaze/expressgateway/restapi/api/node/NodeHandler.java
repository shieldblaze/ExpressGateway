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
package com.shieldblaze.expressgateway.restapi.api.node;

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.NodeBuilder;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.core.cluster.CoreContext;
import com.shieldblaze.expressgateway.core.cluster.LoadBalancerContext;
import com.shieldblaze.expressgateway.restapi.response.FastBuilder;
import com.shieldblaze.expressgateway.restapi.response.builder.APIResponse;
import com.shieldblaze.expressgateway.restapi.response.builder.Result;
import io.netty.handler.codec.http.HttpResponseStatus;
import org.springframework.http.MediaType;
import org.springframework.http.ResponseEntity;
import org.springframework.web.bind.annotation.DeleteMapping;
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.PatchMapping;
import org.springframework.web.bind.annotation.PostMapping;
import org.springframework.web.bind.annotation.PutMapping;
import org.springframework.web.bind.annotation.RequestBody;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.RequestParam;
import org.springframework.web.bind.annotation.RestController;

import java.net.InetSocketAddress;
import java.util.Objects;

@RestController
@RequestMapping("/v1/node")
public class NodeHandler {

    @PostMapping(value = "/create", consumes = MediaType.APPLICATION_JSON_VALUE, produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> create(@RequestParam String id, @RequestParam String clusterHostname, @RequestBody NodeContext nodeContext) throws Exception {
        LoadBalancerContext property = CoreContext.get(id);

        Cluster cluster = property.l4LoadBalancer().cluster(clusterHostname);

        Node node = NodeBuilder.newBuilder()
                .withSocketAddress(new InetSocketAddress(nodeContext.address(), nodeContext.port()))
                .withCluster(cluster)
                .build();

        APIResponse.APIResponseBuilder apiResponseBuilder = APIResponse.newBuilder()
                .isSuccess(node.addedToCluster());

        if (node.addedToCluster()) {
            apiResponseBuilder.withResult(Result.newBuilder().withHeader("NodeID").withMessage(node.id()).build());
        } else {
            throw new IllegalArgumentException("Node cannot be added to Cluster because it already exists in Cluster");
        }

        return FastBuilder.response(apiResponseBuilder.build().getResponse(), HttpResponseStatus.CREATED);
    }

    @DeleteMapping(value = "/delete", produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> delete(@RequestParam String id, @RequestParam String clusterHostname, @RequestParam String nodeId) {
        LoadBalancerContext property = CoreContext.get(id);
        Objects.requireNonNull(clusterHostname, "ClusterHostname");
        Objects.requireNonNull(nodeId, "NodeID");

        Cluster cluster = property.l4LoadBalancer().cluster(clusterHostname);

        Node node = cluster.get(nodeId);
        node.close();

        APIResponse apiResponse = APIResponse.newBuilder()
                .isSuccess(true)
                .build();

        return FastBuilder.response(apiResponse.getResponse(), HttpResponseStatus.OK);
    }

    @PutMapping(value = "/offline", produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> offline(@RequestParam String id, @RequestParam String clusterHostname, @RequestParam String nodeId,
                                          @RequestParam(required = false) boolean drainConnections) {
        LoadBalancerContext property = CoreContext.get(id);
        Objects.requireNonNull(clusterHostname, "ClusterHostname");
        Objects.requireNonNull(nodeId, "NodeID");

        Cluster cluster = property.l4LoadBalancer().cluster(clusterHostname);
        Node node = cluster.get(nodeId);
        boolean success = node.markOffline();

        if (drainConnections) {
            node.drainConnections();
        }

        APIResponse apiResponse = APIResponse.newBuilder()
                .isSuccess(success)
                .build();

        return FastBuilder.response(apiResponse.getResponse(), HttpResponseStatus.OK);
    }

    @PatchMapping(value = "/drainConnections", produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> drainConnections(@RequestParam String id, @RequestParam String clusterHostname, @RequestParam String nodeId) {
        LoadBalancerContext property = CoreContext.get(id);
        Objects.requireNonNull(clusterHostname, "ClusterHostname");
        Objects.requireNonNull(nodeId, "NodeID");

        Cluster cluster = property.l4LoadBalancer().cluster(clusterHostname);

        Node node = cluster.get(nodeId);
        node.drainConnections();

        APIResponse apiResponse = APIResponse.newBuilder()
                .isSuccess(true)
                .build();

        return FastBuilder.response(apiResponse.getResponse(), HttpResponseStatus.OK);
    }

    @PatchMapping(value = "/maxConnections", produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> maxConnections(@RequestParam String id, @RequestParam String clusterHostname, @RequestParam String nodeId,
                                                 @RequestParam int maxConnections) {
        LoadBalancerContext property = CoreContext.get(id);
        Objects.requireNonNull(clusterHostname, "ClusterHostname");
        Objects.requireNonNull(nodeId, "NodeID");

        Cluster cluster = property.l4LoadBalancer().cluster(clusterHostname);

        Node node = cluster.get(nodeId);
        node.maxConnections(maxConnections);

        APIResponse apiResponse = APIResponse.newBuilder()
                .isSuccess(true)
                .build();

        return FastBuilder.response(apiResponse.getResponse(), HttpResponseStatus.OK);
    }

    @GetMapping(produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> get(@RequestParam String id, @RequestParam String clusterHostname, @RequestParam String nodeId) {
        LoadBalancerContext property = CoreContext.get(id);
        Objects.requireNonNull(clusterHostname, "ClusterHostname");
        Objects.requireNonNull(nodeId, "NodeID");

        Cluster cluster = property.l4LoadBalancer().cluster(clusterHostname);
        Node node = cluster.get(nodeId);

        APIResponse apiResponse = APIResponse.newBuilder()
                .isSuccess(true)
                .withResult(Result.newBuilder().withHeader("Node").withMessage(node.toJson()).build())
                .build();

        return FastBuilder.response(apiResponse.getResponse(), HttpResponseStatus.OK);
    }
}
