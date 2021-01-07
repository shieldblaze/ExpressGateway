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
package com.shieldblaze.expressgateway.restapi.loadbalancer.http;

import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterPool;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.HTTPBalance;
import com.shieldblaze.expressgateway.concurrent.eventstream.EventStream;
import com.shieldblaze.expressgateway.configuration.EventStreamConfiguration;
import com.shieldblaze.expressgateway.configuration.HTTPConfiguration;
import com.shieldblaze.expressgateway.configuration.Transformer;
import com.shieldblaze.expressgateway.core.LoadBalancersRegistry;
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancer;
import com.shieldblaze.expressgateway.protocol.http.DefaultHTTPServerInitializer;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancer;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancerBuilder;
import com.shieldblaze.expressgateway.protocol.tcp.TCPListener;
import com.shieldblaze.expressgateway.restapi.response.FastBuilder;
import com.shieldblaze.expressgateway.restapi.response.builder.APIResponse;
import com.shieldblaze.expressgateway.restapi.response.builder.ErrorBase;
import com.shieldblaze.expressgateway.restapi.response.builder.Message;
import com.shieldblaze.expressgateway.restapi.response.builder.Result;
import io.netty.handler.codec.http.HttpResponseStatus;
import io.swagger.v3.oas.annotations.Operation;
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

import java.net.InetAddress;
import java.net.InetSocketAddress;

@RestController
@RequestMapping("{profile}/loadbalancer/http")
@Tag(name = "HTTP Load Balancer", description = "HTTP Load Balancer API")
public class HTTPLoadBalancerHandler {

    @Operation(summary = "Create new Load Balancer", description = "Create and start a new Load Balancer")
    @PostMapping(value = "/create", consumes = MediaType.APPLICATION_JSON_VALUE, produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> create(@PathVariable String profile, @RequestBody HTTPLoadBalancerContext httpLoadBalancerContext) {
        try {

            EventStream eventStream = ((EventStreamConfiguration) Transformer.read(EventStreamConfiguration.EMPTY_INSTANCE, "profile")).eventStream();
            HTTPBalance httpBalance = Utils.determineAlgorithm(httpLoadBalancerContext);
            Cluster cluster = new ClusterPool(eventStream, httpBalance);

            HTTPLoadBalancer httpLoadBalancer = HTTPLoadBalancerBuilder.newBuilder()
                    .withBindAddress(new InetSocketAddress(InetAddress.getByName(httpLoadBalancerContext.bindAddress()), httpLoadBalancerContext.bindPort()))
                    .withCoreConfiguration(Utils.coreConfiguration(profile))
                    .withL4FrontListener(new TCPListener())
                    .withCluster(cluster)
                    .withTLSForServer(httpLoadBalancerContext.tlsForServer() ? Utils.tlsForServer(profile) : null)
                    .withTLSForClient(httpLoadBalancerContext.tlsForClient() ? Utils.tlsForClient(profile) : null)
                    .withHTTPConfiguration((HTTPConfiguration) Transformer.read(HTTPConfiguration.EMPTY_INSTANCE, profile))
                    .withHTTPInitializer(new DefaultHTTPServerInitializer())
                    .build();

            LoadBalancersRegistry.addLoadBalancer(httpLoadBalancer);
            httpLoadBalancer.start();

            APIResponse apiResponse = APIResponse.newBuilder()
                    .isSuccess(true)
                    .withResult(Result.newBuilder().withHeader("LoadBalancerID").withMessage(httpLoadBalancer.ID).build())
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
