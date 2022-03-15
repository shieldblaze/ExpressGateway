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
package com.shieldblaze.expressgateway.restapi.api.configuration;

import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import com.shieldblaze.expressgateway.configuration.transport.TransportConfiguration;
import com.shieldblaze.expressgateway.restapi.CustomOkHttpClient;
import com.shieldblaze.expressgateway.restapi.RestAPI;
import okhttp3.OkHttpClient;
import okhttp3.Request;
import okhttp3.RequestBody;
import okhttp3.Response;
import org.junit.jupiter.api.*;

import java.io.IOException;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

@TestMethodOrder(MethodOrderer.OrderAnnotation.class)
class TransportTest {

    @BeforeAll
    static void startSpring() {
        RestAPI.start();
    }

    @AfterAll
    static void teardown() {
        RestAPI.stop();
    }

    @Order(1)
    @Test
    void applyConfiguration() throws IOException {
        JsonArray receiveSizes = new JsonArray();
        receiveSizes.add(512);

        JsonObject jsonBody = new JsonObject();
        jsonBody.addProperty("transportType", "NIO");
        jsonBody.addProperty("receiveBufferAllocationType", "FIXED");
        jsonBody.add("receiveBufferSizes", receiveSizes);
        jsonBody.addProperty("tcpConnectionBacklog", 50_000);
        jsonBody.addProperty("socketReceiveBufferSize", 67_108_864);
        jsonBody.addProperty("socketSendBufferSize", 67_108_864);
        jsonBody.addProperty("tcpFastOpenMaximumPendingRequests", 100_000);
        jsonBody.addProperty("backendConnectTimeout", 1000 * 10);
        jsonBody.addProperty("connectionIdleTimeout", 1000 * 120);

        Request request = new Request.Builder()
                .url("https://127.0.0.1:9110/v1/configuration/meow/transport/save")
                .post(RequestBody.create(jsonBody.toString().getBytes()))
                .header("Content-Type", "application/json")
                .build();

        try (Response response = CustomOkHttpClient.INSTANCE.newCall(request).execute()) {
            assertNotNull(response.body());
            JsonObject responseJson = JsonParser.parseString(response.body().string()).getAsJsonObject();
            assertTrue(responseJson.get("Success").getAsBoolean());
        }
    }

    @Order(2)
    @Test
    void applyBadConfiguration() throws IOException {
        JsonArray receiveSizes = new JsonArray();
        receiveSizes.add(512);
        receiveSizes.add(1024);
        receiveSizes.add(2048);
        receiveSizes.add(4096); // Maximum sizes are 3

        JsonObject jsonBody = new JsonObject();
        jsonBody.addProperty("transportType", "NIO");
        jsonBody.addProperty("receiveBufferAllocationType", "FIXED");
        jsonBody.add("receiveBufferSizes", receiveSizes);
        jsonBody.addProperty("tcpConnectionBacklog", 50_000);
        jsonBody.addProperty("socketReceiveBufferSize", 67_108_864);
        jsonBody.addProperty("socketSendBufferSize", 67_108_864);
        jsonBody.addProperty("tcpFastOpenMaximumPendingRequests", 100_000);
        jsonBody.addProperty("backendConnectTimeout", 1000 * 10);
        jsonBody.addProperty("connectionIdleTimeout", 1000 * 120);

        Request request = new Request.Builder()
                .url("https://127.0.0.1:9110/v1/configuration/meow/transport/save")
                .post(RequestBody.create(jsonBody.toString().getBytes()))
                .header("Content-Type", "application/json")
                .build();

        try (Response response = CustomOkHttpClient.INSTANCE.newCall(request).execute()) {
            assertNotNull(response.body());
            JsonObject responseJson = JsonParser.parseString(response.body().string()).getAsJsonObject();
            assertFalse(responseJson.get("Success").getAsBoolean());
        }
    }

    @Order(3)
    @Test
    void getDefaultConfiguration() throws IOException {
        TransportConfiguration transportDefault = TransportConfiguration.DEFAULT;

        Request request = new Request.Builder()
                .url("https://127.0.0.1:9110/v1/configuration/default/transport/get")
                .get()
                .build();

        try (Response response = CustomOkHttpClient.INSTANCE.newCall(request).execute()) {
            assertNotNull(response.body());
            JsonObject responseJson = JsonParser.parseString(response.body().string()).getAsJsonObject();
            assertTrue(responseJson.get("Success").getAsBoolean());
            JsonObject bufferObject = responseJson.get("Result").getAsJsonObject().get("TransportConfiguration").getAsJsonObject();

            JsonArray sizes = bufferObject.get("receiveBufferSizes").getAsJsonArray();
            int[] receiveSizes = new int[]{sizes.get(0).getAsInt(), sizes.get(1).getAsInt(), sizes.get(2).getAsInt()};

            assertEquals(transportDefault.transportType().toString(), bufferObject.get("transportType").getAsString());
            assertEquals(transportDefault.receiveBufferAllocationType().toString(), bufferObject.get("receiveBufferAllocationType").getAsString());
            assertArrayEquals(transportDefault.receiveBufferSizes(), receiveSizes);
            assertEquals(transportDefault.tcpConnectionBacklog(), bufferObject.get("tcpConnectionBacklog").getAsInt());
            assertEquals(transportDefault.socketReceiveBufferSize(), bufferObject.get("socketReceiveBufferSize").getAsInt());
            assertEquals(transportDefault.socketSendBufferSize(), bufferObject.get("socketSendBufferSize").getAsInt());
            assertEquals(transportDefault.tcpFastOpenMaximumPendingRequests(), bufferObject.get("tcpFastOpenMaximumPendingRequests").getAsInt());
            assertEquals(transportDefault.backendConnectTimeout(), bufferObject.get("backendConnectTimeout").getAsInt());
            assertEquals(transportDefault.connectionIdleTimeout(), bufferObject.get("connectionIdleTimeout").getAsInt());
        }
    }

    @Order(4)
    @Test
    void getConfiguration() throws IOException {
        JsonArray receiveSizes = new JsonArray();
        receiveSizes.add(512);
        receiveSizes.add(1024);
        receiveSizes.add(2048);

        JsonObject jsonBody = new JsonObject();
        jsonBody.addProperty("transportType", "NIO");
        jsonBody.addProperty("receiveBufferAllocationType", "ADAPTIVE");
        jsonBody.add("receiveBufferSizes", receiveSizes);
        jsonBody.addProperty("tcpConnectionBacklog", 50_000);
        jsonBody.addProperty("socketReceiveBufferSize", 67_108_864);
        jsonBody.addProperty("socketSendBufferSize", 67_108_864);
        jsonBody.addProperty("tcpFastOpenMaximumPendingRequests", 100_000);
        jsonBody.addProperty("backendConnectTimeout", 1000 * 10);
        jsonBody.addProperty("connectionIdleTimeout", 1000 * 120);

        Request request = new Request.Builder()
                .url("https://127.0.0.1:9110/v1/configuration/meow/transport/save")
                .post(RequestBody.create(jsonBody.toString().getBytes()))
                .header("Content-Type", "application/json")
                .build();

        try (Response response = CustomOkHttpClient.INSTANCE.newCall(request).execute()) {
            assertNotNull(response.body());
            JsonObject responseJson = JsonParser.parseString(response.body().string()).getAsJsonObject();
            assertTrue(responseJson.get("Success").getAsBoolean());
        }

        request = new Request.Builder()
                .url("https://127.0.0.1:9110/v1/configuration/meow/transport/get")
                .get()
                .build();

        try (Response response = CustomOkHttpClient.INSTANCE.newCall(request).execute()) {
            assertNotNull(response.body());
            JsonObject responseJson = JsonParser.parseString(response.body().string()).getAsJsonObject();
            assertTrue(responseJson.get("Success").getAsBoolean());
            JsonObject bufferObject = responseJson.get("Result").getAsJsonObject().get("TransportConfiguration").getAsJsonObject();

            JsonArray sizesA = jsonBody.get("receiveBufferSizes").getAsJsonArray();
            int[] receiveSizesIntA = new int[]{sizesA.get(0).getAsInt(), sizesA.get(1).getAsInt(), sizesA.get(2).getAsInt()};

            JsonArray sizesB = bufferObject.get("receiveBufferSizes").getAsJsonArray();
            int[] receiveSizesIntB = new int[]{sizesB.get(0).getAsInt(), sizesB.get(1).getAsInt(), sizesB.get(2).getAsInt()};

            assertEquals(jsonBody.get("transportType").getAsString(), bufferObject.get("transportType").getAsString());
            assertEquals(jsonBody.get("receiveBufferAllocationType").getAsString(), bufferObject.get("receiveBufferAllocationType").getAsString());
            assertArrayEquals(receiveSizesIntA, receiveSizesIntB);
            assertEquals(jsonBody.get("tcpConnectionBacklog").getAsInt(), bufferObject.get("tcpConnectionBacklog").getAsInt());
            assertEquals(jsonBody.get("socketReceiveBufferSize").getAsInt(), bufferObject.get("socketReceiveBufferSize").getAsInt());
            assertEquals(jsonBody.get("socketSendBufferSize").getAsInt(), bufferObject.get("socketSendBufferSize").getAsInt());
            assertEquals(jsonBody.get("tcpFastOpenMaximumPendingRequests").getAsInt(), bufferObject.get("tcpFastOpenMaximumPendingRequests").getAsInt());
            assertEquals(jsonBody.get("backendConnectTimeout").getAsInt(), bufferObject.get("backendConnectTimeout").getAsInt());
            assertEquals(jsonBody.get("connectionIdleTimeout").getAsInt(), bufferObject.get("connectionIdleTimeout").getAsInt());
        }
    }
}
