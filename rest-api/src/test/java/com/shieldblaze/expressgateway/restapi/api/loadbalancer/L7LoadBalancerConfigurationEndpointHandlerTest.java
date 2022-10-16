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

import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import com.shieldblaze.expressgateway.common.ExpressGateway;
import com.shieldblaze.expressgateway.common.crypto.cryptostore.CryptoEntry;
import com.shieldblaze.expressgateway.common.curator.Curator;
import com.shieldblaze.expressgateway.common.utils.SelfSignedCertificate;
import com.shieldblaze.expressgateway.restapi.CustomOkHttpClient;
import com.shieldblaze.expressgateway.restapi.RestApi;
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
import java.security.cert.X509Certificate;
import java.util.List;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

@TestMethodOrder(MethodOrderer.OrderAnnotation.class)
public class L7LoadBalancerConfigurationEndpointHandlerTest {

    private static final RequestBody EMPTY_REQ_BODY = RequestBody.create(new byte[0], null);
    public static String id;

    @BeforeAll
    static void startSpring() throws IOException {
        ExpressGateway expressGateway = ExpressGatewayConfigured.forTest();
        ExpressGateway.setInstance(expressGateway);

        Curator.init();
        RestApi.start();
    }

    @AfterAll
    static void teardown() throws InterruptedException {
        RestApi.stop();
        Thread.sleep(2500);
    }

    @Test
    @Order(1)
    public void startLoadBalancer() throws IOException {
        JsonObject requestBody = new JsonObject();
        requestBody.addProperty("name", "MeowBalancer");
        requestBody.addProperty("bindAddress", "127.0.0.1");
        requestBody.addProperty("bindPort", 50002);

        Request request = new Request.Builder()
                .url("https://127.0.0.1:9110/v1/loadbalancer/l7/http/start")
                .post(RequestBody.create(requestBody.toString(), MediaType.get("application/json")))
                .build();

        try (Response response = CustomOkHttpClient.INSTANCE.newCall(request).execute()) {
            assertNotNull(response.body());
            JsonObject responseJson = JsonParser.parseString(response.body().string()).getAsJsonObject();
            System.out.println(responseJson);
            assertTrue(responseJson.get("Success").getAsBoolean());

            id = responseJson.get("Result").getAsJsonObject().get("LoadBalancerID").getAsString();
        }
    }

    @Test
    @Order(2)
    public void verifyRunning() throws IOException, InterruptedException {
        Thread.sleep(1000); // Wait for Load Balancer to completely start

        Request request = new Request.Builder()
                .url("https://127.0.0.1:9110/v1/loadbalancer/get?id=" + id)
                .get()
                .build();

        try (Response response = CustomOkHttpClient.INSTANCE.newCall(request).execute()) {
            assertNotNull(response.body());
            JsonObject responseJson = JsonParser.parseString(response.body().string()).getAsJsonObject();
            System.out.println(responseJson);
            assertTrue(responseJson.get("Success").getAsBoolean());

            //"Result":{"LoadBalancer":{"ID":"72c8493a-2a6c-43c8-9c48-193b42dd858d","Name":"MeowBalancer","State":"Running","Clusters":[]}}}
            assertEquals("Running", responseJson
                    .get("Result").getAsJsonObject()
                    .get("LoadBalancer").getAsJsonObject()
                    .get("State").getAsString());
        }
    }

    @Test
    @Order(3)
    public void stopLoadBalancer() throws IOException {
        Request request = new Request.Builder()
                .url("https://127.0.0.1:9110/v1/loadbalancer/stop?id=" + id)
                .put(EMPTY_REQ_BODY)
                .build();

        try (Response response = CustomOkHttpClient.INSTANCE.newCall(request).execute()) {
            assertNotNull(response.body());
            JsonObject responseJson = JsonParser.parseString(response.body().string()).getAsJsonObject();
            System.out.println(responseJson);
            assertTrue(responseJson.get("Success").getAsBoolean());
        }
    }

    @Test
    @Order(4)
    public void resumeLoadBalancer() throws IOException {
        Request request = new Request.Builder()
                .url("https://127.0.0.1:9110/v1/loadbalancer/resume?id=" + id)
                .put(EMPTY_REQ_BODY)
                .build();

        try (Response response = CustomOkHttpClient.INSTANCE.newCall(request).execute()) {
            assertNotNull(response.body());
            JsonObject responseJson = JsonParser.parseString(response.body().string()).getAsJsonObject();
            System.out.println(responseJson);
            assertTrue(responseJson.get("Success").getAsBoolean());
        }
    }

    @Test
    @Order(5)
    public void shutdownLoadBalancer() throws IOException {
        Request request = new Request.Builder()
                .url("https://127.0.0.1:9110/v1/loadbalancer/shutdown?id=" + id)
                .delete()
                .build();

        try (Response response = CustomOkHttpClient.INSTANCE.newCall(request).execute()) {
            assertNotNull(response.body());
            JsonObject responseJson = JsonParser.parseString(response.body().string()).getAsJsonObject();
            System.out.println(responseJson);
            assertTrue(responseJson.get("Success").getAsBoolean());
        }
    }

    @Test
    @Order(6)
    void nullLoadBalancerIDTest() throws IOException {
        JsonObject requestBody = new JsonObject();
        requestBody.addProperty("name", "MeowBalancer");
        requestBody.addProperty("bindAddress", "127.0.0.1");
        requestBody.addProperty("bindPort", 50003);
        requestBody.addProperty("protocol", "tcp");


        Request request = new Request.Builder()
                .url("https://127.0.0.1:9110/v1/loadbalancer/l7/http/start")
                .post(RequestBody.create(requestBody.toString().getBytes()))
                .header("Content-Type", "application/json")
                .build();

        try (Response response = CustomOkHttpClient.INSTANCE.newCall(request).execute()) {
            assertNotNull(response.body());
            JsonObject responseJson = JsonParser.parseString(response.body().string()).getAsJsonObject();
            System.out.println(responseJson);
            assertTrue(responseJson.get("Success").getAsBoolean());

            id = responseJson.get("Result").getAsJsonObject().get("LoadBalancerID").getAsString();
        }

        // -------------------------------------- STOP, RESUME AND SHUTDOWN ------------------------------------------

        request = new Request.Builder()
                .url("https://127.0.0.1:9110/v1/loadbalancer/stop?id=")
                .put(EMPTY_REQ_BODY)
                .build();

        try (Response response = CustomOkHttpClient.INSTANCE.newCall(request).execute()) {
            assertNotNull(response.body());
            JsonObject responseJson = JsonParser.parseString(response.body().string()).getAsJsonObject();
            System.out.println(responseJson);
            assertFalse(responseJson.get("Success").getAsBoolean());
        }

        request = new Request.Builder()
                .url("https://127.0.0.1:9110/v1/loadbalancer/resume?id=")
                .put(EMPTY_REQ_BODY)
                .build();

        try (Response response = CustomOkHttpClient.INSTANCE.newCall(request).execute()) {
            assertNotNull(response.body());
            JsonObject responseJson = JsonParser.parseString(response.body().string()).getAsJsonObject();
            System.out.println(responseJson);
            assertFalse(responseJson.get("Success").getAsBoolean());
        }

        request = new Request.Builder()
                .url("https://127.0.0.1:9110/v1/loadbalancer/shutdown?id=")
                .delete()
                .build();

        try (Response response = CustomOkHttpClient.INSTANCE.newCall(request).execute()) {
            assertNotNull(response.body());
            JsonObject responseJson = JsonParser.parseString(response.body().string()).getAsJsonObject();
            System.out.println(responseJson);
            assertFalse(responseJson.get("Success").getAsBoolean());
        }

        // Actual Shutdown
        request = new Request.Builder()
                .url("https://127.0.0.1:9110/v1/loadbalancer/shutdown?id=" + id)
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
