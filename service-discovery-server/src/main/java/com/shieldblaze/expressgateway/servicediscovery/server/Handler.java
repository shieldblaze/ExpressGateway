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
import com.shieldblaze.expressgateway.common.utils.LogSanitizer;
import lombok.RequiredArgsConstructor;
import lombok.extern.slf4j.Slf4j;
import org.apache.curator.x.discovery.ServiceDiscovery;
import org.apache.curator.x.discovery.ServiceInstance;
import org.apache.curator.x.discovery.ServiceInstanceBuilder;
import org.springframework.http.HttpStatus;
import org.springframework.http.MediaType;
import org.springframework.http.ResponseEntity;
import org.springframework.web.bind.annotation.DeleteMapping;
import org.springframework.web.bind.annotation.ExceptionHandler;
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

@Slf4j
@RestController
@RequestMapping("/api/v1/service")
@RequiredArgsConstructor
public class Handler {

    private final ServiceDiscovery<Node> serviceDiscovery;
    private final RegistrationStore registrationStore;
    private final HealthAggregator healthAggregator;

    @PutMapping(value = "register", produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> register(@RequestBody Node node) throws Exception {
        serviceDiscovery.registerService(instance(node));
        registrationStore.register(node, 0);

        ObjectNode objectNode = OBJECT_MAPPER.createObjectNode();
        objectNode.put("Success", true);

        return ResponseEntity.status(HttpStatus.OK).body(objectNode.toPrettyString());
    }

    @DeleteMapping(value = "deregister", produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> deregister(@RequestBody Node node) throws Exception {
        serviceDiscovery.unregisterService(instance(node));
        registrationStore.deregister(node.id());

        ObjectNode objectNode = OBJECT_MAPPER.createObjectNode();
        objectNode.put("Success", true);

        return ResponseEntity.status(HttpStatus.OK).body(objectNode.toPrettyString());
    }

    @GetMapping(value = "get", produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> get(@RequestParam("id") String id) throws Exception {
        ServiceInstance<Node> serviceInstance = serviceDiscovery.queryForInstance(SERVICE_NAME, id);

        ObjectNode objectNode = OBJECT_MAPPER.createObjectNode();
        if (serviceInstance == null) {
            objectNode.put("Success", false);
            objectNode.put("Error", "Instance not found: " + LogSanitizer.sanitize(id));
            return ResponseEntity.status(HttpStatus.NOT_FOUND).body(objectNode.toPrettyString());
        }

        objectNode.put("Success", true);
        ArrayNode arrayNode = objectNode.putArray("Instances");
        arrayNode.addPOJO(serviceInstance);

        return ResponseEntity.status(HttpStatus.OK).body(objectNode.toPrettyString());
    }

    @GetMapping(value = "getall", produces = MediaType.APPLICATION_JSON_VALUE)
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

    // ---- Bulk Operations ----

    @PutMapping(value = "bulk/register", produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> bulkRegister(@RequestBody BulkRequest request) throws Exception {
        request.validate();

        int registered = 0;
        for (Node node : request.nodes()) {
            serviceDiscovery.registerService(instance(node));
            registrationStore.register(node, request.ttlSeconds());
            registered++;
        }

        ObjectNode objectNode = OBJECT_MAPPER.createObjectNode();
        objectNode.put("Success", true);
        objectNode.put("Registered", registered);

        return ResponseEntity.status(HttpStatus.OK).body(objectNode.toPrettyString());
    }

    @DeleteMapping(value = "bulk/deregister", produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> bulkDeregister(@RequestBody BulkRequest request) throws Exception {
        request.validate();

        int deregistered = 0;
        for (Node node : request.nodes()) {
            serviceDiscovery.unregisterService(instance(node));
            registrationStore.deregister(node.id());
            deregistered++;
        }

        ObjectNode objectNode = OBJECT_MAPPER.createObjectNode();
        objectNode.put("Success", true);
        objectNode.put("Deregistered", deregistered);

        return ResponseEntity.status(HttpStatus.OK).body(objectNode.toPrettyString());
    }

    // ---- Health Endpoints ----

    @PostMapping(value = "heartbeat", produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> heartbeat(@RequestParam("id") String id) {
        boolean updated = healthAggregator.processHeartbeat(id);

        ObjectNode objectNode = OBJECT_MAPPER.createObjectNode();
        objectNode.put("Success", updated);
        if (!updated) {
            objectNode.put("Error", "Node not found: " + LogSanitizer.sanitize(id));
        }

        return ResponseEntity.status(updated ? HttpStatus.OK : HttpStatus.NOT_FOUND)
                .body(objectNode.toPrettyString());
    }

    @GetMapping(value = "health", produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> healthSummary() {
        HealthAggregator.HealthSummary summary = healthAggregator.summarize();

        ObjectNode objectNode = OBJECT_MAPPER.createObjectNode();
        objectNode.put("Success", true);
        objectNode.put("Total", summary.total());
        objectNode.put("Healthy", summary.healthy());
        objectNode.put("Unhealthy", summary.unhealthy());
        objectNode.put("Expired", summary.expired());

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

    @ExceptionHandler(IllegalArgumentException.class)
    public ResponseEntity<String> handleValidationError(IllegalArgumentException ex) {
        log.warn("Validation error: {}", ex.getMessage());
        ObjectNode objectNode = OBJECT_MAPPER.createObjectNode();
        objectNode.put("Success", false);
        objectNode.put("Error", ex.getMessage());
        return ResponseEntity.status(HttpStatus.BAD_REQUEST).body(objectNode.toPrettyString());
    }

    @ExceptionHandler(Exception.class)
    public ResponseEntity<String> handleGenericError(Exception ex) {
        log.error("Internal error in service discovery handler", ex);
        ObjectNode objectNode = OBJECT_MAPPER.createObjectNode();
        objectNode.put("Success", false);
        objectNode.put("Error", "Internal server error");
        return ResponseEntity.status(HttpStatus.INTERNAL_SERVER_ERROR).body(objectNode.toPrettyString());
    }
}
