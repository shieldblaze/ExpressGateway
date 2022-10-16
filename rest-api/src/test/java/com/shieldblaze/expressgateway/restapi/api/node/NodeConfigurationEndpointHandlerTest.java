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
package com.shieldblaze.expressgateway.restapi.api.node;

import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import com.shieldblaze.expressgateway.backend.State;
import com.shieldblaze.expressgateway.common.ExpressGateway;
import com.shieldblaze.expressgateway.common.curator.Curator;
import com.shieldblaze.expressgateway.core.cluster.CoreContext;
import com.shieldblaze.expressgateway.core.cluster.LoadBalancerContext;
import com.shieldblaze.expressgateway.restapi.CustomOkHttpClient;
import com.shieldblaze.expressgateway.restapi.RestApi;
import com.shieldblaze.expressgateway.restapi.api.cluster.ClusterConfigurationEndpointHandlerTest;
import com.shieldblaze.expressgateway.restapi.api.loadbalancer.L4LoadBalancerConfigurationEndpointHandlerTest;
import com.shieldblaze.expressgateway.testing.ExpressGatewayConfigured;
import okhttp3.MediaType;
import okhttp3.Request;
import okhttp3.RequestBody;
import okhttp3.Response;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.MethodOrderer;
import org.junit.jupiter.api.Order;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.TestMethodOrder;

import java.io.IOException;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

@TestMethodOrder(MethodOrderer.OrderAnnotation.class)
class NodeConfigurationEndpointHandlerTest {

    private static final RequestBody EMPTY_REQ_BODY = RequestBody.create(new byte[0], null);
    private static final ClusterConfigurationEndpointHandlerTest clusterHandlerTest = new ClusterConfigurationEndpointHandlerTest();
    private static String nodeId;

    @BeforeAll
    static void startSpring() throws IOException, InterruptedException {
        ExpressGateway expressGateway = ExpressGatewayConfigured.forTest();
        ExpressGateway.setInstance(expressGateway);

        Curator.init();
        RestApi.start();
        clusterHandlerTest.addL4ClusterTest();
    }

    @AfterAll
    static void teardown() throws IOException, InterruptedException {
        clusterHandlerTest.deleteL4ClusterTest();
        RestApi.stop();
    }

    @Test
    @Order(1)
    void createNode() throws IOException {
        JsonObject body = new JsonObject();
        body.addProperty("address", "127.0.0.1");
        body.addProperty("port", 54321);

        Request request = new Request.Builder()
                .url("https://127.0.0.1:9110/v1/node/create?id=" + L4LoadBalancerConfigurationEndpointHandlerTest.id + "&clusterHostname=default")
                .post(RequestBody.create(body.toString(), MediaType.get("application/json")))
                .build();

        try (Response response = CustomOkHttpClient.INSTANCE.newCall(request).execute()) {
            assertNotNull(response.body());
            JsonObject responseJson = JsonParser.parseString(response.body().string()).getAsJsonObject();
            System.out.println(responseJson);
            assertTrue(responseJson.get("Success").getAsBoolean());

            nodeId = responseJson.get("Result").getAsJsonObject().get("NodeID").getAsString();
        }
    }

    @Test
    @Order(2)
    void markManuallyOfflineTest() throws IOException {
        Request request = new Request.Builder()
                .url("https://127.0.0.1:9110/v1/node/offline?id=" + L4LoadBalancerConfigurationEndpointHandlerTest.id + "&clusterHostname=default&nodeId=" + nodeId)
                .put(EMPTY_REQ_BODY)
                .build();

        try (Response response = CustomOkHttpClient.INSTANCE.newCall(request).execute()) {
            assertNotNull(response.body());
            JsonObject responseJson = JsonParser.parseString(response.body().string()).getAsJsonObject();
            System.out.println(responseJson);
            assertTrue(responseJson.get("Success").getAsBoolean());
        }

        LoadBalancerContext property = CoreContext.get(L4LoadBalancerConfigurationEndpointHandlerTest.id);
        assertEquals(State.MANUAL_OFFLINE, property.l4LoadBalancer().cluster("default").get(nodeId).state());
    }

    @Test
    @Order(3)
    void changeMaxConnectionsTest() throws IOException {
        LoadBalancerContext property = CoreContext.get(L4LoadBalancerConfigurationEndpointHandlerTest.id);
        assertEquals(10_000, property.l4LoadBalancer().cluster("default").get(nodeId).maxConnections());

        Request request = new Request.Builder()
                .url("https://127.0.0.1:9110/v1/node/maxConnections?id=" + L4LoadBalancerConfigurationEndpointHandlerTest.id + "&clusterHostname=default&nodeId=" + nodeId + "&maxConnections=1000000")
                .patch(EMPTY_REQ_BODY)
                .build();

        try (Response response = CustomOkHttpClient.INSTANCE.newCall(request).execute()) {
            assertNotNull(response.body());
            JsonObject responseJson = JsonParser.parseString(response.body().string()).getAsJsonObject();
            System.out.println(responseJson);
            assertTrue(responseJson.get("Success").getAsBoolean());
        }

        assertEquals(1_000_000, property.l4LoadBalancer().cluster("default").get(nodeId).maxConnections());
    }

    @Test
    @Order(4)
    void getNodeTest() throws IOException {
        Request request = new Request.Builder()
                .url("https://127.0.0.1:9110/v1/node/?id=" + L4LoadBalancerConfigurationEndpointHandlerTest.id + "&clusterHostname=default&nodeId=" + nodeId)
                .get()
                .build();

        try (Response response = CustomOkHttpClient.INSTANCE.newCall(request).execute()) {
            assertNotNull(response.body());
            JsonObject responseJson = JsonParser.parseString(response.body().string()).getAsJsonObject();
            System.out.println(responseJson);
            assertTrue(responseJson.get("Success").getAsBoolean());
            assertEquals(nodeId, responseJson.get("Result").getAsJsonObject().get("Node").getAsJsonObject().get("ID").getAsString());
        }
    }

    @Test
    void deleteNodeTest() throws IOException {
        Request request = new Request.Builder()
                .url("https://127.0.0.1:9110/v1/node/delete?id=" + L4LoadBalancerConfigurationEndpointHandlerTest.id + "&clusterHostname=default&nodeId=" + nodeId)
                .delete()
                .build();

        try (Response response = CustomOkHttpClient.INSTANCE.newCall(request).execute()) {
            assertNotNull(response.body());
            JsonObject responseJson = JsonParser.parseString(response.body().string()).getAsJsonObject();
            System.out.println(responseJson);
            assertTrue(responseJson.get("Success").getAsBoolean());
        }
    }
}
