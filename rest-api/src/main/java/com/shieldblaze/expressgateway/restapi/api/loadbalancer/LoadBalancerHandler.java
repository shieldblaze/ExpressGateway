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
package com.shieldblaze.expressgateway.restapi.api.loadbalancer;

import com.shieldblaze.expressgateway.configuration.ConfigurationContext;
import com.shieldblaze.expressgateway.core.L4FrontListener;
import com.shieldblaze.expressgateway.core.cluster.CoreContext;
import com.shieldblaze.expressgateway.core.cluster.LoadBalancerContext;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStartupEvent;
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancer;
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancerBuilder;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancer;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancerBuilder;
import com.shieldblaze.expressgateway.protocol.tcp.TCPListener;
import com.shieldblaze.expressgateway.protocol.udp.UDPListener;
import com.shieldblaze.expressgateway.restapi.exceptions.InvalidLoadBalancerStartRequestException;
import com.shieldblaze.expressgateway.restapi.response.FastBuilder;
import com.shieldblaze.expressgateway.restapi.response.builder.APIResponse;
import com.shieldblaze.expressgateway.restapi.response.builder.Result;
import io.netty.handler.codec.http.HttpResponseStatus;
import org.springframework.http.MediaType;
import org.springframework.http.ResponseEntity;
import org.springframework.web.bind.annotation.DeleteMapping;
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.PostMapping;
import org.springframework.web.bind.annotation.PutMapping;
import org.springframework.web.bind.annotation.RequestBody;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.RequestParam;
import org.springframework.web.bind.annotation.RestController;

import java.net.InetSocketAddress;

/**
 * <p> LoadBalancerHandler takes care of fetching registered load balancers,
 * creating new load balancers (L4/L7), stopping load balancers, resuming
 * load balancers and shutting down load balancers. </p>
 */
@RestController
@RequestMapping("/v1/loadbalancer")
public final class LoadBalancerHandler {

    /**
     * This method will start a L4 Load Balancer (TCP /UDP).
     *
     * @param ctx {@link LoadBalancerStartContext} instance
     * @return {@link ResponseEntity} containing result response
     */
    @PostMapping(value = "/l4/start", consumes = MediaType.APPLICATION_JSON_VALUE, produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> startL4LoadBalancer(@RequestBody LoadBalancerStartContext ctx) {

        // Determine the L4 Protocol (TCP/UDP)
        L4FrontListener l4FrontListener;
        if (ctx.protocol() != null && ctx.protocol().equalsIgnoreCase("tcp")) {
            l4FrontListener = new TCPListener();
        } else if (ctx.protocol() != null && ctx.protocol().equalsIgnoreCase("udp")) {
            l4FrontListener = new UDPListener();
        } else {
            // If Protocol is not 'TCP' or 'UDP' then throw error.
            throw new IllegalArgumentException("Invalid L4 Protocol: " + ctx.protocol() + "; Expected: TCP/UDP");
        }

        // Create a new Load Balancer
        L4LoadBalancer l4LoadBalancer = L4LoadBalancerBuilder.newBuilder()
                .withBindAddress(new InetSocketAddress(ctx.bindAddress(), ctx.bindPort()))
                .withL4FrontListener(l4FrontListener)
                .withCoreConfiguration(ConfigurationContext.DEFAULT)
                .withName(ctx.name())
                .build();

        // Register the event, so we can query it later
        // for its status, for reboot or shutdown.
        L4FrontListenerStartupEvent event = l4LoadBalancer.start();
        CoreContext.add(l4LoadBalancer.id(), new LoadBalancerContext(l4LoadBalancer, event));

        // Build the API call response
        APIResponse apiResponse = APIResponse.newBuilder()
                .isSuccess(true)
                .withResult(Result.newBuilder().withHeader("LoadBalancerID").withMessage(l4LoadBalancer.id()).build())
                .build();

        return FastBuilder.response(apiResponse.getResponse(), HttpResponseStatus.CREATED);
    }

    @PostMapping(value = "/l7/http/start", consumes = MediaType.APPLICATION_JSON_VALUE, produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> start(@RequestBody LoadBalancerStartContext loadBalancerStartContext) {

        HTTPLoadBalancer httpLoadBalancer = HTTPLoadBalancerBuilder.newBuilder()
                .withBindAddress(new InetSocketAddress(loadBalancerStartContext.bindAddress(), loadBalancerStartContext.bindPort()))
                .withL4FrontListener(new TCPListener())
                .withConfigurationContext(ConfigurationContext.DEFAULT)
                .withName(loadBalancerStartContext.name())
                .build();

        L4FrontListenerStartupEvent event = httpLoadBalancer.start();
        CoreContext.add(httpLoadBalancer.id(), new LoadBalancerContext(httpLoadBalancer, event));

        APIResponse apiResponse = APIResponse.newBuilder()
                .isSuccess(true)
                .withResult(Result.newBuilder().withHeader("LoadBalancerID").withMessage(httpLoadBalancer.id()).build())
                .build();

        return FastBuilder.response(apiResponse.getResponse(), HttpResponseStatus.CREATED);
    }

    @PutMapping(value = "/resume", produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> resume(@RequestParam String id) {
        LoadBalancerContext context = CoreContext.get(id);

        L4FrontListenerStartupEvent event = context.l4LoadBalancer().start();
        context.modifyStartupEvent(event);

        APIResponse apiResponse = APIResponse.newBuilder()
                .isSuccess(true)
                .build();

        return FastBuilder.response(apiResponse.getResponse(), HttpResponseStatus.CREATED);
    }

    @PutMapping(value = "/stop", produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> stop(@RequestParam String id) {
        LoadBalancerContext property = CoreContext.get(id);
        property.l4LoadBalancer().stop();

        APIResponse apiResponse = APIResponse.newBuilder()
                .isSuccess(true)
                .build();

        return FastBuilder.response(apiResponse.getResponse(), HttpResponseStatus.OK);
    }

    @DeleteMapping(value = "/shutdown", produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> shutdown(@RequestParam String id) {
        LoadBalancerContext property = CoreContext.get(id);
        CoreContext.remove(id);

        property.l4LoadBalancer().shutdown();

        APIResponse apiResponse = APIResponse.newBuilder()
                .isSuccess(true)
                .build();

        return FastBuilder.response(apiResponse.getResponse(), HttpResponseStatus.OK);
    }

    @GetMapping(value = "/get", produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> get(@RequestParam String id) {
        LoadBalancerContext property = CoreContext.get(id);

        // If Load Balancer startup has finished and is not successful,
        // then remove mapping of that Load Balancer.
        if (property.modifyStartupEvent().isFinished() && !property.modifyStartupEvent().isSuccess()) {
            CoreContext.remove(property.l4LoadBalancer().id());
        }

        APIResponse apiResponse = APIResponse.newBuilder()
                .isSuccess(true)
                .withResult(Result.newBuilder().withHeader("LoadBalancer").withMessage(property.l4LoadBalancer().toJson()).build())
                .build();

        return FastBuilder.response(apiResponse.getResponse(), HttpResponseStatus.OK);
    }
}
