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
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStartupEvent;
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancer;
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancerBuilder;
import com.shieldblaze.expressgateway.core.registry.LoadBalancerProperty;
import com.shieldblaze.expressgateway.core.registry.LoadBalancerRegistry;
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
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.PostMapping;
import org.springframework.web.bind.annotation.RequestBody;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.RequestParam;
import org.springframework.web.bind.annotation.RestController;

import java.net.InetSocketAddress;

@RestController
@RequestMapping("/v1/loadbalancer")
public final class LoadBalancer {

    @PostMapping(value = "/l4/start", consumes = MediaType.APPLICATION_JSON_VALUE, produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> startL4(@RequestBody LoadBalancerStartStruct loadBalancerStartStruct) {
        L4FrontListener l4FrontListener;
        if (loadBalancerStartStruct.protocol().equalsIgnoreCase("tcp")) {
            l4FrontListener = new TCPListener();
        } else if (loadBalancerStartStruct.protocol().equalsIgnoreCase("udp")) {
            l4FrontListener = new UDPListener();
        } else {
            // If Protocol is not 'TCP' or 'UDP" then throw error.
            throw new IllegalArgumentException("Invalid L4 Protocol");
        }

        TLSConfiguration tlsForClient = null;
        if (loadBalancerStartStruct.tlsForClient()) {
            tlsForClient = TLSConfiguration.loadClient();
        }

        TLSConfiguration tlsForServer = null;
        if (loadBalancerStartStruct.tlsForServer()) {
            tlsForServer = TLSConfiguration.loadServer();
        }

        L4LoadBalancer l4LoadBalancer = L4LoadBalancerBuilder.newBuilder()
                .withBindAddress(new InetSocketAddress(loadBalancerStartStruct.bindAddress(), loadBalancerStartStruct.bindPort()))
                .withL4FrontListener(l4FrontListener)
                .withCoreConfiguration(CoreConfiguration.INSTANCE)
                .withName(loadBalancerStartStruct.name())
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
    public ResponseEntity<String> start(@RequestBody LoadBalancerStartStruct loadBalancerStartStruct) {
        L4FrontListener l4FrontListener;
        if (loadBalancerStartStruct.protocol().equalsIgnoreCase("http")) {
            l4FrontListener = new TCPListener();
        } else {
            // If Protocol is not 'TCP' or 'UDP" then throw error.
            throw new IllegalArgumentException("Invalid L7 Protocol");
        }

        TLSConfiguration tlsForClient = null;
        if (loadBalancerStartStruct.tlsForClient()) {
            tlsForClient = TLSConfiguration.loadClient();
        }

        TLSConfiguration tlsForServer = null;
        if (loadBalancerStartStruct.tlsForServer()) {
            tlsForServer = TLSConfiguration.loadServer();
        }

        HTTPLoadBalancer httpLoadBalancer = HTTPLoadBalancerBuilder.newBuilder()
                .withBindAddress(new InetSocketAddress("0.0.0.0", 9110))
                .withL4FrontListener(new TCPListener())
                .build();

        L4LoadBalancer l4LoadBalancer = L4LoadBalancerBuilder.newBuilder()
                .withBindAddress(new InetSocketAddress(loadBalancerStartStruct.bindAddress(), loadBalancerStartStruct.bindPort()))
                .withL4FrontListener(l4FrontListener)
                .withCoreConfiguration(CoreConfiguration.INSTANCE)
                .withName(loadBalancerStartStruct.name())
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

    @GetMapping(value = "/get", produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> get(@RequestParam String id) {
        LoadBalancerProperty property = LoadBalancerRegistry.get(id);

        if (property == null) {
            throw new NullPointerException("LoadBalancer not found with the ID: " + id);
        }

        APIResponse apiResponse = APIResponse.newBuilder()
                .isSuccess(true)
                .withResult(Result.newBuilder().withHeader("LoadBalancer").withMessage(property.l4LoadBalancer().toJson()).build())
                .build();

        return FastBuilder.response(apiResponse.getResponse(), HttpResponseStatus.OK);
    }
}
