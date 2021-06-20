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
package com.shieldblaze.expressgateway.restapi.api.cluster;

import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import com.shieldblaze.expressgateway.core.registry.LoadBalancerProperty;
import com.shieldblaze.expressgateway.core.registry.LoadBalancerRegistry;
import com.shieldblaze.expressgateway.restapi.RestAPI;
import com.shieldblaze.expressgateway.restapi.api.loadbalancer.L4LoadBalancerTest;
import com.shieldblaze.expressgateway.restapi.api.loadbalancer.L7LoadBalancerTest;
import okhttp3.MediaType;
import okhttp3.OkHttpClient;
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

import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

@TestMethodOrder(MethodOrderer.OrderAnnotation.class)
public class ClusterHandlerTest {

    private static final RequestBody EMPTY_REQ_BODY = RequestBody.create(new byte[0], null);
    private static final OkHttpClient OK_HTTP_CLIENT = new OkHttpClient();
    private static final L4LoadBalancerTest l4LoadBalancerTest = new L4LoadBalancerTest();
    private static final L7LoadBalancerTest l7LoadBalancerTest = new L7LoadBalancerTest();

    @BeforeAll
    static void startSpring() {
        RestAPI.start();
    }

    @AfterAll
    static void teardown() throws IOException, InterruptedException {
        l4LoadBalancerTest.shutdownLoadBalancer();
        OK_HTTP_CLIENT.dispatcher().cancelAll();
        RestAPI.stop();
        Thread.sleep(2500);
    }

    @Test
    @Order(1)
    public void addL4ClusterTest() throws IOException, InterruptedException {
        l4LoadBalancerTest.startLoadBalancer();
        l4LoadBalancerTest.verifyRunning();

        final LoadBalancerProperty property = LoadBalancerRegistry.get(L4LoadBalancerTest.id);
        assertThrows(NullPointerException.class, () -> property.l4LoadBalancer().cluster("DEFAULT"));

        JsonObject body = new JsonObject();
        body.addProperty("Hostname", "www.shieldblaze.com"); // It will default down to 'DEFAULT'.
        body.addProperty("LoadBalance", "RoundRobin");
        body.addProperty("SessionPersistence", "NOOP");

        Request request = new Request.Builder()
                .url("http://127.0.0.1:9110/v1/cluster/create?id=" + L4LoadBalancerTest.id)
                .post(RequestBody.create(body.toString(), MediaType.get("application/json")))
                .build();

        try (Response response = OK_HTTP_CLIENT.newCall(request).execute()) {
            assertNotNull(response.body());
            JsonObject responseJson = JsonParser.parseString(response.body().string()).getAsJsonObject();
            assertTrue(responseJson.get("Success").getAsBoolean());
        }

        assertNotNull(property.l4LoadBalancer().cluster("DEFAULT"));
    }

    @Test
    @Order(2)
    public void deleteL4ClusterTest() throws IOException {
        LoadBalancerProperty property = LoadBalancerRegistry.get(L4LoadBalancerTest.id);
        assertNotNull(property.l4LoadBalancer().cluster("DEFAULT"));

        Request request = new Request.Builder()
                .url("http://127.0.0.1:9110/v1/cluster/delete?id=" + L4LoadBalancerTest.id + "&hostname=null")
                .delete()
                .build();

        try (Response response = OK_HTTP_CLIENT.newCall(request).execute()) {
            assertNotNull(response.body());
            JsonObject responseJson = JsonParser.parseString(response.body().string()).getAsJsonObject();
            assertTrue(responseJson.get("Success").getAsBoolean());
        }

        assertThrows(NullPointerException.class, () -> property.l4LoadBalancer().cluster("DEFAULT"));
    }

    @Test
    @Order(3)
    public void addL7ClusterTest() throws IOException, InterruptedException {
        l7LoadBalancerTest.startLoadBalancer();
        l7LoadBalancerTest.verifyRunning();

        LoadBalancerProperty property = LoadBalancerRegistry.get(L7LoadBalancerTest.id);
        assertThrows(NullPointerException.class, () -> property.l4LoadBalancer().cluster("www.shieldblaze.com"));

        JsonObject body = new JsonObject();
        body.addProperty("Hostname", "www.shieldblaze.com");
        body.addProperty("LoadBalance", "HTTPRoundRobin");
        body.addProperty("SessionPersistence", "NOOP");

        Request request = new Request.Builder()
                .url("http://127.0.0.1:9110/v1/cluster/create?id=" + L7LoadBalancerTest.id)
                .post(RequestBody.create(body.toString(), MediaType.get("application/json")))
                .build();

        try (Response response = OK_HTTP_CLIENT.newCall(request).execute()) {
            assertNotNull(response.body());
            JsonObject responseJson = JsonParser.parseString(response.body().string()).getAsJsonObject();
            assertTrue(responseJson.get("Success").getAsBoolean());
        }

        assertNotNull(property.l4LoadBalancer().cluster("www.shieldblaze.com"));
    }

    @Test
    @Order(4)
    public void remapL7ClusterTest() throws IOException {
        LoadBalancerProperty property = LoadBalancerRegistry.get(L7LoadBalancerTest.id);
        assertNotNull(property.l4LoadBalancer().cluster("www.shieldblaze.com"));
        assertThrows(NullPointerException.class, () -> property.l4LoadBalancer().cluster("shieldblaze.com"));

        Request request = new Request.Builder()
                .url("http://127.0.0.1:9110/v1/cluster/remap?id=" + L7LoadBalancerTest.id + "&oldHostname=www.shieldblaze.com&newHostname=shieldblaze.com")
                .put(EMPTY_REQ_BODY)
                .build();

        try (Response response = OK_HTTP_CLIENT.newCall(request).execute()) {
            assertNotNull(response.body());
            JsonObject responseJson = JsonParser.parseString(response.body().string()).getAsJsonObject();
            assertTrue(responseJson.get("Success").getAsBoolean());
        }

        assertThrows(NullPointerException.class, () -> property.l4LoadBalancer().cluster("www.shieldblaze.com"));
        assertNotNull(property.l4LoadBalancer().cluster("shieldblaze.com"));
    }

    @Test
    @Order(5)
    public void deleteL7ClusterTest() throws IOException {
        Request request = new Request.Builder()
                .url("http://127.0.0.1:9110/v1/cluster/delete?id=" + L7LoadBalancerTest.id + "&hostname=shieldblaze.com")
                .delete()
                .build();

        try (Response response = OK_HTTP_CLIENT.newCall(request).execute()) {
            assertNotNull(response.body());
            JsonObject responseJson = JsonParser.parseString(response.body().string()).getAsJsonObject();
            assertTrue(responseJson.get("Success").getAsBoolean());
        }
    }
}
