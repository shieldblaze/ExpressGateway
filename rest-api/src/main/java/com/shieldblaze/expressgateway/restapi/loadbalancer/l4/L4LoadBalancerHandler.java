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
package com.shieldblaze.expressgateway.restapi.loadbalancer.l4;

import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterPool;
import com.shieldblaze.expressgateway.backend.strategy.l4.L4Balance;
import com.shieldblaze.expressgateway.concurrent.eventstream.EventStream;
import com.shieldblaze.expressgateway.configuration.EventStreamConfiguration;
import com.shieldblaze.expressgateway.configuration.Transformer;
import com.shieldblaze.expressgateway.core.LoadBalancersRegistry;
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancer;
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancerBuilder;
import com.shieldblaze.expressgateway.restapi.response.FastBuilder;
import com.shieldblaze.expressgateway.restapi.response.builder.APIResponse;
import com.shieldblaze.expressgateway.restapi.response.builder.ErrorBase;
import com.shieldblaze.expressgateway.restapi.response.builder.Message;
import com.shieldblaze.expressgateway.restapi.response.builder.Result;
import io.netty.handler.codec.http.HttpResponseStatus;
import io.swagger.v3.oas.annotations.Operation;
import io.swagger.v3.oas.annotations.Parameter;
import io.swagger.v3.oas.annotations.tags.Tag;
import org.springframework.context.annotation.Description;
import org.springframework.http.HttpStatus;
import org.springframework.http.MediaType;
import org.springframework.http.ResponseEntity;
import org.springframework.web.bind.annotation.DeleteMapping;
import org.springframework.web.bind.annotation.PathVariable;
import org.springframework.web.bind.annotation.PostMapping;
import org.springframework.web.bind.annotation.RequestBody;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.RestController;

import java.net.InetAddress;
import java.net.InetSocketAddress;

@RestController
@RequestMapping("{profile}/loadbalancer/{protocol}")
@Tag(name = "Layer-4 Load Balancer", description = "Layer-4 Load Balancer API")
public class L4LoadBalancerHandler {

    @Operation(summary = "Create new Load Balancer", description = "Create and start a new Load Balancer")
    @PostMapping(path = "/create", consumes = MediaType.APPLICATION_JSON_VALUE, produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> create(@Parameter(description = "Profile name")
                                         @PathVariable String profile,
                                         //--------------------------------------
                                         @Parameter(description = "Protocol to be Load Balanced (TCP/UDP)")
                                         @PathVariable String protocol,
                                         //--------------------------------------
                                         @Parameter(description = "Request Body containing all information for creation of Load Balancer")
                                         @RequestBody L4LoadBalancerContext l4LoadBalancerContext) {
        try {
            EventStream eventStream = ((EventStreamConfiguration) Transformer.read(EventStreamConfiguration.EMPTY_INSTANCE, "profile")).eventStream();
            L4Balance l4Balance = Utils.determineAlgorithm(l4LoadBalancerContext);
            Cluster cluster = new ClusterPool(eventStream, l4Balance);

            L4LoadBalancer l4LoadBalancer = L4LoadBalancerBuilder.newBuilder()
                    .withL4FrontListener(Utils.determineListener(protocol))
                    .withBindAddress(new InetSocketAddress(InetAddress.getByName(l4LoadBalancerContext.bindAddress()), l4LoadBalancerContext.bindPort()))
                    .withCluster(cluster)
                    .withCoreConfiguration(Utils.coreConfiguration(profile))
                    .withTLSForServer(l4LoadBalancerContext.tlsForServer() ? Utils.tlsForServer(profile) : null)
                    .withTLSForClient(l4LoadBalancerContext.tlsForClient() ? Utils.tlsForClient(profile) : null)
                    .build();

            LoadBalancersRegistry.addLoadBalancer(l4LoadBalancer);
            l4LoadBalancer.start();

            APIResponse apiResponse = APIResponse.newBuilder()
                    .isSuccess(true)
                    .withResult(Result.newBuilder().withHeader("LoadBalancerID").withMessage(l4LoadBalancer.ID).build())
                    .build();

            return new ResponseEntity<>(apiResponse.response(), HttpStatus.CREATED);
        } catch (Exception ex) {
            return FastBuilder.error(ErrorBase.REQUEST_ERROR, Message.newBuilder()
                    .withHeader("Error")
                    .withMessage(ex.getLocalizedMessage())
                    .build(), HttpResponseStatus.BAD_REQUEST);
        }
    }

    @Operation(summary = "Stop a Load Balancer", description = "Stop and destroy a Load Balancer")
    @DeleteMapping(path = "/stop/{id}", consumes = MediaType.APPLICATION_JSON_VALUE, produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> stop(@PathVariable("id") String id) {
        try {
            L4LoadBalancer l4LoadBalancer = LoadBalancersRegistry.id(id);

            // If LoadBalancer is not found then return error.
            if (l4LoadBalancer == null) {
                return FastBuilder.error(ErrorBase.LOADBALANCER_NOT_FOUND, HttpResponseStatus.NOT_FOUND);
            }

            l4LoadBalancer.stop();
            l4LoadBalancer.cluster().eventStream().close();
            LoadBalancersRegistry.removeLoadBalancer(l4LoadBalancer);

            APIResponse apiResponse = APIResponse.newBuilder()
                    .isSuccess(true)
                    .withResult(Result.newBuilder().withHeader("LoadBalancerID").withMessage(l4LoadBalancer.ID).build())
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
