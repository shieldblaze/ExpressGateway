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
package com.shieldblaze.expressgateway.restapi.api.node;

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.NodeBuilder;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.core.registry.LoadBalancerProperty;
import com.shieldblaze.expressgateway.core.registry.LoadBalancerRegistry;
import com.shieldblaze.expressgateway.restapi.response.FastBuilder;
import com.shieldblaze.expressgateway.restapi.response.builder.APIResponse;
import com.shieldblaze.expressgateway.restapi.response.builder.Result;
import io.netty.handler.codec.http.HttpResponseStatus;
import org.springframework.http.MediaType;
import org.springframework.http.ResponseEntity;
import org.springframework.web.bind.annotation.PostMapping;
import org.springframework.web.bind.annotation.RequestBody;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.RequestParam;
import org.springframework.web.bind.annotation.RestController;

import java.net.InetSocketAddress;
import java.net.UnknownHostException;

@RestController
@RequestMapping("/v1/node")
public class NodeHandler {

    @PostMapping(value = "/create", consumes = MediaType.APPLICATION_JSON_VALUE, produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> create(@RequestParam String id, @RequestBody NodeStruct nodeStruct) throws UnknownHostException {
        LoadBalancerProperty property = LoadBalancerRegistry.get(id);

        Cluster cluster = property.l4LoadBalancer().cluster(nodeStruct.clusterHostname());
        if (cluster == null) {
            throw new NullPointerException("Cluster not found with Hostname: " + nodeStruct.clusterHostname());
        }

        Node node = NodeBuilder.newBuilder()
                .withSocketAddress(new InetSocketAddress(nodeStruct.address(), nodeStruct.port()))
                .withCluster(cluster)
                .build();

        APIResponse apiResponse = APIResponse.newBuilder()
                .isSuccess(true)
                .withResult(Result.newBuilder().withHeader("NodeID").withMessage(node.id()).build())
                .build();

        return FastBuilder.response(apiResponse.getResponse(), HttpResponseStatus.CREATED);
    }
}
