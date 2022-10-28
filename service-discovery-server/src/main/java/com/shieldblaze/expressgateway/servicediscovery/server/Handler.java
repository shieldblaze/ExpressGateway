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
package com.shieldblaze.expressgateway.servicediscovery.server;

import com.fasterxml.jackson.databind.node.ArrayNode;
import com.fasterxml.jackson.databind.node.ObjectNode;
import org.apache.curator.x.discovery.ServiceDiscovery;
import org.apache.curator.x.discovery.ServiceInstance;
import org.apache.curator.x.discovery.ServiceInstanceBuilder;
import org.springframework.beans.factory.annotation.Autowired;
import org.springframework.http.HttpStatus;
import org.springframework.http.ResponseEntity;
import org.springframework.web.bind.annotation.DeleteMapping;
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.PostMapping;
import org.springframework.web.bind.annotation.PutMapping;
import org.springframework.web.bind.annotation.RequestBody;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.RequestParam;
import org.springframework.web.bind.annotation.RestController;

import java.util.Collection;

import static com.shieldblaze.expressgateway.common.JacksonJson.OBJECT_MAPPER;
import static com.shieldblaze.expressgateway.servicediscovery.server.ServiceDiscoveryServer.SERVICE_NAME;
import static io.netty.handler.codec.http.HttpHeaders.Values.APPLICATION_JSON;

@RestController
@RequestMapping("/api/v1/service/")
public class Handler {

    private final ServiceDiscovery<Node> serviceDiscovery;

    public Handler(ServiceDiscovery<Node> serviceDiscovery) {
        this.serviceDiscovery = serviceDiscovery;
    }

    @PutMapping(value = "/register", produces = APPLICATION_JSON)
    public ResponseEntity<String> register(@RequestBody Node node) throws Exception {
        serviceDiscovery.registerService(instance(node));

        ObjectNode objectNode = OBJECT_MAPPER.createObjectNode();
        objectNode.put("Success", true);

        return ResponseEntity.status(HttpStatus.OK).body(objectNode.toPrettyString());
    }

    @DeleteMapping(value = "/unregister", produces = APPLICATION_JSON)
    public ResponseEntity<String> unregister(@RequestBody Node node) throws Exception {
        serviceDiscovery.unregisterService(instance(node));

        ObjectNode objectNode = OBJECT_MAPPER.createObjectNode();
        objectNode.put("Success", true);

        return ResponseEntity.status(HttpStatus.OK).body(objectNode.toPrettyString());
    }

    @GetMapping(value = "/get", produces = APPLICATION_JSON)
    public ResponseEntity<String> get(@RequestParam String id) throws Exception {
        ServiceInstance<Node> serviceInstance = serviceDiscovery.queryForInstance(SERVICE_NAME, id);

        ObjectNode objectNode = OBJECT_MAPPER.createObjectNode();
        objectNode.put("Success", true);
        ArrayNode arrayNode = objectNode.putArray("Instances");
        arrayNode.addPOJO(serviceInstance);

        return ResponseEntity.status(HttpStatus.OK).body(objectNode.toPrettyString());
    }

    @GetMapping(value = "/getall", produces = APPLICATION_JSON)
    public ResponseEntity<String> getAll() throws Exception {
        Collection<ServiceInstance<Node>> serviceInstances = serviceDiscovery.queryForInstances(SERVICE_NAME);

        ObjectNode objectNode = OBJECT_MAPPER.createObjectNode();
        objectNode.put("Success", true);
        ArrayNode arrayNode = objectNode.putArray("Instances");

        for (ServiceInstance<Node> instance : serviceInstances) {
            arrayNode.addPOJO(instance);
        }

        return ResponseEntity.status(HttpStatus.OK).body(objectNode.toPrettyString());
    }

    static ServiceInstance<Node> instance(Node node) throws Exception {
        node.validate();
        ServiceInstanceBuilder<Node> serviceInstanceBuilder = ServiceInstance.<Node>builder()
                .name("ExpressGateway")
                .id(node.id())
                .address(node.ipAddress())
                .sslPort(-1)
                .port(-1)
                .payload(node);

        if (node.tlsEnabled()) {
            serviceInstanceBuilder.sslPort(node.port());
        } else {
            serviceInstanceBuilder.port(node.port());
        }

        return serviceInstanceBuilder.build();
    }
}
