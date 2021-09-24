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
package com.shieldblaze.expressgateway.restapi.api.loadbalancer;

import com.shieldblaze.expressgateway.configuration.CoreConfiguration;
import com.shieldblaze.expressgateway.configuration.tls.TLSConfiguration;
import com.shieldblaze.expressgateway.core.L4FrontListener;
import com.shieldblaze.expressgateway.core.cluster.LoadBalancerProperty;
import com.shieldblaze.expressgateway.core.cluster.LoadBalancerRegistry;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStartupEvent;
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancer;
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancerBuilder;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancer;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancerBuilder;
import com.shieldblaze.expressgateway.protocol.tcp.TCPListener;
import com.shieldblaze.expressgateway.protocol.udp.UDPListener;
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

@RestController
@RequestMapping("/v1/loadbalancer")
public final class LoadBalancer {

    @PostMapping(value = "/l4/start", consumes = MediaType.APPLICATION_JSON_VALUE, produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> startL4(@RequestBody LoadBalancerContext loadBalancerContext) {
        L4FrontListener l4FrontListener;
        if (loadBalancerContext.protocol() != null && loadBalancerContext.protocol().equalsIgnoreCase("tcp")) {
            l4FrontListener = new TCPListener();
        } else if (loadBalancerContext.protocol() != null && loadBalancerContext.protocol().equalsIgnoreCase("udp")) {
            l4FrontListener = new UDPListener();
        } else {
            // If Protocol is not 'TCP' or 'UDP" then throw error.
            throw new IllegalArgumentException("Invalid L4 Protocol");
        }

        TLSConfiguration tlsForClient = null;
        if (loadBalancerContext.tlsForClient()) {
            tlsForClient = TLSConfiguration.loadClient();
        }

        TLSConfiguration tlsForServer = null;
        if (loadBalancerContext.tlsForServer()) {
            tlsForServer = TLSConfiguration.loadServer();
        }

        L4LoadBalancer l4LoadBalancer = L4LoadBalancerBuilder.newBuilder()
                .withBindAddress(new InetSocketAddress(loadBalancerContext.bindAddress(), loadBalancerContext.bindPort()))
                .withL4FrontListener(l4FrontListener)
                .withCoreConfiguration(CoreConfiguration.INSTANCE)
                .withName(loadBalancerContext.name())
                .withTLSForClient(tlsForClient)
                .withTLSForServer(tlsForServer)
                .build();

        L4FrontListenerStartupEvent event = l4LoadBalancer.start();
        LoadBalancerRegistry.add(l4LoadBalancer.ID, new LoadBalancerProperty(l4LoadBalancer, event));

        APIResponse apiResponse = APIResponse.newBuilder()
                .isSuccess(true)
                .withResult(Result.newBuilder().withHeader("LoadBalancerID").withMessage(l4LoadBalancer.ID).build())
                .build();

        return FastBuilder.response(apiResponse.getResponse(), HttpResponseStatus.CREATED);
    }

    @PostMapping(value = "/l7/http/start", consumes = MediaType.APPLICATION_JSON_VALUE, produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> start(@RequestBody LoadBalancerContext loadBalancerContext) {

        TLSConfiguration tlsForClient = null;
        if (loadBalancerContext.tlsForClient()) {
            tlsForClient = TLSConfiguration.loadClient();
        }

        TLSConfiguration tlsForServer = null;
        if (loadBalancerContext.tlsForServer()) {
            tlsForServer = TLSConfiguration.loadServer();
        }

        HTTPLoadBalancer httpLoadBalancer = HTTPLoadBalancerBuilder.newBuilder()
                .withBindAddress(new InetSocketAddress(loadBalancerContext.bindAddress(), loadBalancerContext.bindPort()))
                .withL4FrontListener(new TCPListener())
                .withCoreConfiguration(CoreConfiguration.INSTANCE)
                .withName(loadBalancerContext.name())
                .withTLSForClient(tlsForClient)
                .withTLSForServer(tlsForServer)
                .build();

        L4FrontListenerStartupEvent event = httpLoadBalancer.start();
        LoadBalancerRegistry.add(httpLoadBalancer.ID, new LoadBalancerProperty(httpLoadBalancer, event));

        APIResponse apiResponse = APIResponse.newBuilder()
                .isSuccess(true)
                .withResult(Result.newBuilder().withHeader("LoadBalancerID").withMessage(httpLoadBalancer.ID).build())
                .build();

        return FastBuilder.response(apiResponse.getResponse(), HttpResponseStatus.CREATED);
    }

    @PutMapping(value = "/resume", produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> resume(@RequestParam String id) {
        LoadBalancerProperty property = LoadBalancerRegistry.get(id);

        L4FrontListenerStartupEvent event = property.l4LoadBalancer().start();
        property.startupEvent(event);

        APIResponse apiResponse = APIResponse.newBuilder()
                .isSuccess(true)
                .build();

        return FastBuilder.response(apiResponse.getResponse(), HttpResponseStatus.CREATED);
    }

    @PutMapping(value = "/stop", produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> stop(@RequestParam String id) {
        LoadBalancerProperty property = LoadBalancerRegistry.get(id);
        property.l4LoadBalancer().stop();

        APIResponse apiResponse = APIResponse.newBuilder()
                .isSuccess(true)
                .build();

        return FastBuilder.response(apiResponse.getResponse(), HttpResponseStatus.OK);
    }

    @DeleteMapping(value = "/shutdown", produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> shutdown(@RequestParam String id) {
        LoadBalancerProperty property = LoadBalancerRegistry.get(id);
        LoadBalancerRegistry.remove(id);

        property.l4LoadBalancer().shutdown();

        APIResponse apiResponse = APIResponse.newBuilder()
                .isSuccess(true)
                .build();

        return FastBuilder.response(apiResponse.getResponse(), HttpResponseStatus.OK);
    }

    @GetMapping(value = "/get", produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> get(@RequestParam String id) {
        LoadBalancerProperty property = LoadBalancerRegistry.get(id);

        // If Load Balancer startup has finished and is not successful,
        // then remove mapping of that Load Balancer.
        if (property.startupEvent().isFinished() && !property.startupEvent().isSuccess()) {
            LoadBalancerRegistry.remove(property.l4LoadBalancer().ID);
        }

        APIResponse apiResponse = APIResponse.newBuilder()
                .isSuccess(true)
                .withResult(Result.newBuilder().withHeader("LoadBalancer").withMessage(property.l4LoadBalancer().toJson()).build())
                .build();

        return FastBuilder.response(apiResponse.getResponse(), HttpResponseStatus.OK);
    }
}
