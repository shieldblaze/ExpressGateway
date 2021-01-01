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
package com.shieldblaze.expressgateway.restapi.stats;

import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.common.GSON;
import com.shieldblaze.expressgateway.core.LoadBalancersRegistry;
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancer;
import com.shieldblaze.expressgateway.restapi.response.FastBuilder;
import com.shieldblaze.expressgateway.restapi.response.builder.ErrorBase;
import com.shieldblaze.expressgateway.restapi.response.builder.Message;
import io.netty.handler.codec.http.HttpResponseStatus;
import io.swagger.v3.oas.annotations.Operation;
import io.swagger.v3.oas.annotations.tags.Tag;
import org.springframework.http.HttpStatus;
import org.springframework.http.MediaType;
import org.springframework.http.ResponseEntity;
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.PathVariable;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.RestController;

@RestController
@RequestMapping("/stats")
@Tag(name = "Statistics", description = "Statistics of Load Balancer(s)")
public class Stats {

    @Operation(summary = "Get Stats of Everything", description = "Get Stats of Nodes across all Load Balancers")
    @GetMapping(value = "/all", produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> all() {
        try {

            JsonObject mainObject = new JsonObject();
            JsonArray statsArray = new JsonArray();

            for (L4LoadBalancer l4LoadBalancer : LoadBalancersRegistry.getAll()) {

                // Load Balancer Stats
                JsonObject jsonObject = new JsonObject();
                jsonObject.addProperty("ID", l4LoadBalancer.ID);
                jsonObject.addProperty("SocketAddress", l4LoadBalancer.bindAddress().toString());
                jsonObject.addProperty("Running", l4LoadBalancer.running().get());

                // Cluster Stats
                JsonArray clusterArray = new JsonArray();
                for (Node node : l4LoadBalancer.cluster().nodes()) {
                    JsonObject nodeObject = new JsonObject();
                    nodeObject.addProperty("ID", node.id());
                    nodeObject.addProperty("SocketAddress", node.socketAddress().toString());
                    nodeObject.addProperty("BytesSent", node.bytesSent());
                    nodeObject.addProperty("BytesReceived", node.bytesReceived());
                    nodeObject.addProperty("State", node.state().toString());
                    nodeObject.addProperty("Load", node.load());
                    nodeObject.addProperty("Health", node.health().toString());
                    nodeObject.addProperty("Connections", node.activeConnection() + "/" + node.maxConnections());

                    clusterArray.add(nodeObject);
                }

                jsonObject.add("Cluster", clusterArray);
                statsArray.add(jsonObject);
            }

            mainObject.add("Stats", statsArray);

            return new ResponseEntity<>(GSON.INSTANCE.toJson(mainObject.toString()), HttpStatus.OK);
        } catch (Exception ex) {
            return FastBuilder.error(ErrorBase.REQUEST_ERROR, Message.newBuilder()
                    .withHeader("Error")
                    .withMessage(ex.getLocalizedMessage())
                    .build(), HttpResponseStatus.BAD_REQUEST);
        }
    }

    @Operation(summary = "Get Stats of Load Balancer", description = "Get Stats Of Specific Load Balancer")
    @GetMapping(value = "/loadBalancer/{LBID}", produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> loadBalancer(@PathVariable String LBID) {
        try {

            L4LoadBalancer l4LoadBalancer = LoadBalancersRegistry.id(LBID);

            // If LoadBalancer is not found then return error.
            if (l4LoadBalancer == null) {
                return FastBuilder.error(ErrorBase.LOADBALANCER_NOT_FOUND, HttpResponseStatus.NOT_FOUND);
            }

            JsonObject mainObject = new JsonObject();
            JsonArray statsArray = new JsonArray();

            // Load Balancer Stats
            JsonObject jsonObject = new JsonObject();
            jsonObject.addProperty("ID", l4LoadBalancer.ID);
            jsonObject.addProperty("SocketAddress", l4LoadBalancer.bindAddress().toString());
            jsonObject.addProperty("Running", l4LoadBalancer.running().get());

            // Cluster Stats
            JsonArray clusterArray = new JsonArray();
            for (Node node : l4LoadBalancer.cluster().nodes()) {
                JsonObject nodeObject = new JsonObject();
                nodeObject.addProperty("ID", node.id());
                nodeObject.addProperty("SocketAddress", node.socketAddress().toString());
                nodeObject.addProperty("BytesSent", node.bytesSent());
                nodeObject.addProperty("BytesReceived", node.bytesReceived());
                nodeObject.addProperty("State", node.state().toString());
                nodeObject.addProperty("Load", node.load());
                nodeObject.addProperty("Health", node.health().toString());
                nodeObject.addProperty("Connections", node.activeConnection() + "/" + node.maxConnections());

                clusterArray.add(nodeObject);
            }

            jsonObject.add("Cluster", clusterArray);
            statsArray.add(jsonObject);
            mainObject.add("Stats", statsArray);

            return new ResponseEntity<>(GSON.INSTANCE.toJson(mainObject.toString()), HttpStatus.OK);
        } catch (Exception ex) {
            return FastBuilder.error(ErrorBase.REQUEST_ERROR, Message.newBuilder()
                    .withHeader("Error")
                    .withMessage(ex.getLocalizedMessage())
                    .build(), HttpResponseStatus.BAD_REQUEST);
        }
    }
}
