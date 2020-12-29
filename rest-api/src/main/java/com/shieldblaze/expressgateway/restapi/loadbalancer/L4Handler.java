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
package com.shieldblaze.expressgateway.restapi.loadbalancer;

import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterPool;
import com.shieldblaze.expressgateway.backend.strategy.l4.L4Balance;
import com.shieldblaze.expressgateway.concurrent.eventstream.EventStream;
import com.shieldblaze.expressgateway.configuration.transformer.EventStreamTransformer;
import com.shieldblaze.expressgateway.core.LoadBalancersRegistry;
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancer;
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancerBuilder;
import com.shieldblaze.expressgateway.protocol.tcp.TCPListener;
import com.shieldblaze.expressgateway.restapi.response.FastBuilder;
import com.shieldblaze.expressgateway.restapi.response.builder.APIResponse;
import com.shieldblaze.expressgateway.restapi.response.builder.ErrorBase;
import com.shieldblaze.expressgateway.restapi.response.builder.Result;
import io.netty.handler.codec.http.HttpResponseStatus;
import org.springframework.http.HttpStatus;
import org.springframework.http.ResponseEntity;
import org.springframework.web.bind.annotation.PathVariable;
import org.springframework.web.bind.annotation.PostMapping;
import org.springframework.web.bind.annotation.PutMapping;
import org.springframework.web.bind.annotation.RequestBody;
import org.springframework.web.bind.annotation.RestController;

import java.net.InetSocketAddress;

@RestController("/loadbalancer/")
public class L4Handler {

    @PostMapping("/create")
    public ResponseEntity<String> create(@RequestBody L4HandlerContext l4HandlerContext) {
        try {
            EventStream eventStream = EventStreamTransformer.readFile().eventStream();
            L4Balance l4Balance = Utils.determine(l4HandlerContext);
            Cluster cluster = new ClusterPool(eventStream, l4Balance);

            L4LoadBalancer l4LoadBalancer = L4LoadBalancerBuilder.newBuilder()
                    .withL4FrontListener(new TCPListener())
                    .withBindAddress(new InetSocketAddress(l4HandlerContext.bindAddress(), l4HandlerContext.bindPort()))
                    .withCluster(cluster)
                    .withCoreConfiguration(Utils.coreConfiguration())
                    .build();

            LoadBalancersRegistry.addLoadBalancer(l4LoadBalancer);
            l4LoadBalancer.start();

            APIResponse apiResponse = APIResponse.newBuilder()
                    .isSuccess(true)
                    .withResult(Result.newBuilder().withHeader("ID").withMessage(l4LoadBalancer.ID).build())
                    .build();

            return new ResponseEntity<>(apiResponse.response(), HttpStatus.CREATED);
        } catch (Exception ex) {
            return new ResponseEntity<>("Error Occurred: " + ex.getLocalizedMessage(), HttpStatus.BAD_REQUEST);
        }
    }

    @PutMapping("/stop/{id}")
    public ResponseEntity<String> stop(@PathVariable("id") String id) {
        try {
            L4LoadBalancer l4LoadBalancer = LoadBalancersRegistry.id(id);

            // If LoadBalancer is not found then return error.
            if (l4LoadBalancer == null) {
               return FastBuilder.error(ErrorBase.LOADBALANCER_NOT_FOUND, HttpResponseStatus.NOT_FOUND);
            }

            l4LoadBalancer.stop();
            LoadBalancersRegistry.removeLoadBalancer(l4LoadBalancer);

            APIResponse apiResponse = APIResponse.newBuilder()
                    .isSuccess(true)
                    .withResult(Result.newBuilder().withHeader("ID").withMessage(l4LoadBalancer.ID).build())
                    .build();

            return new ResponseEntity<>(apiResponse.response(), HttpStatus.OK);
        } catch (Exception ex) {
            return new ResponseEntity<>("Error Occurred: " + ex.getLocalizedMessage(), HttpStatus.BAD_REQUEST);
        }
    }
}
