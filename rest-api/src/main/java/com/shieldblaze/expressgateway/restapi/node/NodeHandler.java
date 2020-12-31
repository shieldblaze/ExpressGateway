/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020 ShieldBlaze
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
package com.shieldblaze.expressgateway.restapi.node;

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.core.LoadBalancersRegistry;
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancer;
import com.shieldblaze.expressgateway.restapi.response.FastBuilder;
import com.shieldblaze.expressgateway.restapi.response.builder.APIResponse;
import com.shieldblaze.expressgateway.restapi.response.builder.ErrorBase;
import com.shieldblaze.expressgateway.restapi.response.builder.Message;
import com.shieldblaze.expressgateway.restapi.response.builder.Result;
import io.netty.handler.codec.http.HttpResponseStatus;
import io.swagger.v3.oas.annotations.Parameter;
import io.swagger.v3.oas.annotations.tags.Tag;
import org.springframework.http.HttpStatus;
import org.springframework.http.MediaType;
import org.springframework.http.ResponseEntity;
import org.springframework.web.bind.annotation.DeleteMapping;
import org.springframework.web.bind.annotation.PathVariable;
import org.springframework.web.bind.annotation.PostMapping;
import org.springframework.web.bind.annotation.RequestBody;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.RestController;

import java.net.InetSocketAddress;

@RestController
@RequestMapping("/node")
@Tag(name = "Node Handler", description = "Node API")
public class NodeHandler {

    @PostMapping(value = "/{LBID}/add", consumes = MediaType.APPLICATION_JSON_VALUE, produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> addNode(@Parameter(description = "Load Balancer ID")
                                          @PathVariable String LBID,
                                          @Parameter(description = "JSON Body containing Configuration Data")
                                          @RequestBody AddNodeHandler addNodeHandler) {
        try {
            L4LoadBalancer l4LoadBalancer = LoadBalancersRegistry.id(LBID);

            // If LoadBalancer is not found then return error.
            if (l4LoadBalancer == null) {
                return FastBuilder.error(ErrorBase.LOADBALANCER_NOT_FOUND, HttpResponseStatus.NOT_FOUND);
            }

            Node node;
            if (addNodeHandler.healthCheckContext() == null) {
                node = new Node(l4LoadBalancer.cluster(), new InetSocketAddress(addNodeHandler.host(), addNodeHandler.port()), addNodeHandler.maxConnections());
            } else {
                node = new Node(l4LoadBalancer.cluster(), new InetSocketAddress(addNodeHandler.host(), addNodeHandler.port()), addNodeHandler.maxConnections(),
                        Utils.determine(addNodeHandler));
            }

            APIResponse apiResponse = APIResponse.newBuilder()
                    .isSuccess(true)
                    .withResult(Result.newBuilder().withHeader("NodeID").withMessage(node.id()).build())
                    .build();

            return new ResponseEntity<>(apiResponse.response(), HttpStatus.NO_CONTENT);
        } catch (Exception ex) {
            return FastBuilder.error(ErrorBase.REQUEST_ERROR, Message.newBuilder()
                    .withHeader("Error")
                    .withMessage(ex.getLocalizedMessage())
                    .build(), HttpResponseStatus.BAD_REQUEST);
        }
    }

    @DeleteMapping(value = "/{LBID}/remove/{NodeID}", produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> removeNode(@Parameter(description = "Load Balancer ID")
                                             @PathVariable String LBID,
                                             @Parameter(description = "NodeID to be removed")
                                             @PathVariable String NodeID) {
        try {
            L4LoadBalancer l4LoadBalancer = LoadBalancersRegistry.id(LBID);

            // If LoadBalancer is not found then return error.
            if (l4LoadBalancer == null) {
                return FastBuilder.error(ErrorBase.LOADBALANCER_NOT_FOUND, HttpResponseStatus.NOT_FOUND);
            }

            boolean deleteNode = l4LoadBalancer.cluster().removeNode(NodeID);

            APIResponse apiResponse = APIResponse.newBuilder()
                    .isSuccess(deleteNode)
                    .build();

            return new ResponseEntity<>(apiResponse.response(), HttpStatus.OK);
        } catch (Exception ex) {
            return FastBuilder.error(ErrorBase.REQUEST_ERROR, Message.newBuilder()
                    .withHeader("Error")
                    .withMessage(ex.getLocalizedMessage())
                    .build(), HttpResponseStatus.BAD_REQUEST);
        }
    }
}
