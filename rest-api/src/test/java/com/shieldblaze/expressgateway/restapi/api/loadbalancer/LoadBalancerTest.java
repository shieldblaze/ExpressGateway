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

import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import com.shieldblaze.expressgateway.restapi.RestAPI;
import okhttp3.OkHttpClient;
import okhttp3.Request;
import okhttp3.RequestBody;
import okhttp3.Response;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

import java.io.IOException;

import static org.junit.jupiter.api.Assertions.*;

class LoadBalancerTest {

    private static OkHttpClient okHttpClient;

    @BeforeAll
    static void startSpring() {
        RestAPI.start();
        okHttpClient = new OkHttpClient();
    }

    @AfterAll
    static void teardown() {
        okHttpClient.dispatcher().cancelAll();
        RestAPI.stop();
    }

    @Test
    void startStopResumeAndShutdownTCPLoadBalancerTest() throws IOException, InterruptedException {
        JsonObject requestBody = new JsonObject();
        requestBody.addProperty("name", "MeowBalancer");
        requestBody.addProperty("bindAddress", "127.0.0.1");
        requestBody.addProperty("bindPort", 9110);
        requestBody.addProperty("protocol", "tcp");

        RequestBody reqbody = RequestBody.create(new byte[0], null);

        Request request = new Request.Builder()
                .url("http://127.0.0.1:9110/v1/loadbalancer/l4/start")
                .post(RequestBody.create(requestBody.toString().getBytes()))
                .header("Content-Type", "application/json")
                .build();

        String loadBalancerId;

        try (Response response = okHttpClient.newCall(request).execute()) {
            assertNotNull(response.body());
            JsonObject responseJson = JsonParser.parseString(response.body().string()).getAsJsonObject();
            assertTrue(responseJson.get("Success").getAsBoolean());

            loadBalancerId = responseJson.get("Result").getAsJsonObject().get("LoadBalancerID").getAsString();
        }

        // -------------------------------------- STOP ------------------------------------------
        Thread.sleep(5000);

        request = new Request.Builder()
                .url("http://127.0.0.1:9110/v1/loadbalancer/stop?id=" + loadBalancerId)
                .put(reqbody)
                .build();

        try (Response response = okHttpClient.newCall(request).execute()) {
            assertNotNull(response.body());
            JsonObject responseJson = JsonParser.parseString(response.body().string()).getAsJsonObject();
            assertTrue(responseJson.get("Success").getAsBoolean());
        }

        // -------------------------------------- RESUME -----------------------------------------

        Thread.sleep(5000);

        request = new Request.Builder()
                .url("http://127.0.0.1:9110/v1/loadbalancer/resume?id=" + loadBalancerId)
                .put(reqbody)
                .build();

        try (Response response = okHttpClient.newCall(request).execute()) {
            assertNotNull(response.body());
            JsonObject responseJson = JsonParser.parseString(response.body().string()).getAsJsonObject();
            assertTrue(responseJson.get("Success").getAsBoolean());
        }

        // -------------------------------------- SHUTDOWN -----------------------------------------

        Thread.sleep(5000);

        request = new Request.Builder()
                .url("http://127.0.0.1:9110/v1/loadbalancer/shutdown?id=" + loadBalancerId)
                .delete()
                .build();

        try (Response response = okHttpClient.newCall(request).execute()) {
            assertNotNull(response.body());
            JsonObject responseJson = JsonParser.parseString(response.body().string()).getAsJsonObject();
            assertTrue(responseJson.get("Success").getAsBoolean());
        }
    }

    @Test
    void nullLoadBalancerIDTest() throws InterruptedException, IOException {
        JsonObject requestBody = new JsonObject();
        requestBody.addProperty("name", "MeowBalancer");
        requestBody.addProperty("bindAddress", "127.0.0.1");
        requestBody.addProperty("bindPort", 9111);
        requestBody.addProperty("protocol", "tcp");

        RequestBody reqbody = RequestBody.create(new byte[0], null);

        Request request = new Request.Builder()
                .url("http://127.0.0.1:9110/v1/loadbalancer/l4/start")
                .post(RequestBody.create(requestBody.toString().getBytes()))
                .header("Content-Type", "application/json")
                .build();

        String loadBalancerId;

        try (Response response = okHttpClient.newCall(request).execute()) {
            assertNotNull(response.body());
            JsonObject responseJson = JsonParser.parseString(response.body().string()).getAsJsonObject();
            assertTrue(responseJson.get("Success").getAsBoolean());

            loadBalancerId = responseJson.get("Result").getAsJsonObject().get("LoadBalancerID").getAsString();
        }

        // -------------------------------------- STOP, RESUME AND SHUTDOWN ------------------------------------------

        request = new Request.Builder()
                .url("http://127.0.0.1:9110/v1/loadbalancer/stop?id=")
                .put(reqbody)
                .build();

        try (Response response = okHttpClient.newCall(request).execute()) {
            assertNotNull(response.body());
            JsonObject responseJson = JsonParser.parseString(response.body().string()).getAsJsonObject();
            assertFalse(responseJson.get("Success").getAsBoolean());
        }

        request = new Request.Builder()
                .url("http://127.0.0.1:9110/v1/loadbalancer/resume?id=")
                .put(reqbody)
                .build();

        try (Response response = okHttpClient.newCall(request).execute()) {
            assertNotNull(response.body());
            JsonObject responseJson = JsonParser.parseString(response.body().string()).getAsJsonObject();
            assertFalse(responseJson.get("Success").getAsBoolean());
        }

        request = new Request.Builder()
                .url("http://127.0.0.1:9110/v1/loadbalancer/shutdown?id=")
                .delete()
                .build();

        try (Response response = okHttpClient.newCall(request).execute()) {
            assertNotNull(response.body());
            JsonObject responseJson = JsonParser.parseString(response.body().string()).getAsJsonObject();
            assertFalse(responseJson.get("Success").getAsBoolean());
        }

        request = new Request.Builder()
                .url("http://127.0.0.1:9110/v1/loadbalancer/shutdown?id=" + loadBalancerId)
                .delete()
                .build();

        try (Response response = okHttpClient.newCall(request).execute()) {
            assertNotNull(response.body());
            JsonObject responseJson = JsonParser.parseString(response.body().string()).getAsJsonObject();
            assertTrue(responseJson.get("Success").getAsBoolean());
        }
    }
}
