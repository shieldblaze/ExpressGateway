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

import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import com.shieldblaze.expressgateway.common.ExpressGateway;
import com.shieldblaze.expressgateway.common.zookeeper.Curator;
import com.shieldblaze.expressgateway.configuration.buffer.BufferConfiguration;
import com.shieldblaze.expressgateway.restapi.CustomOkHttpClient;
import com.shieldblaze.expressgateway.restapi.RestApi;
import com.shieldblaze.expressgateway.testing.ExpressGatewayConfigured;
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
class BufferTest {

    @BeforeAll
    static void startSpring() throws IOException {
        ExpressGateway expressGateway = ExpressGatewayConfigured.forZooKeeperTest();
        ExpressGateway.setInstance(expressGateway);

        Curator.init();
        RestApi.start();
    }

    @AfterAll
    static void teardown() {
        RestApi.stop();
    }

    @Order(1)
    @Test
    void applyConfigurationTest() throws IOException {
        JsonObject jsonBody = new JsonObject();
        jsonBody.addProperty("preferDirect", true);
        jsonBody.addProperty("heapArena", 12);
        jsonBody.addProperty("directArena", 12);
        jsonBody.addProperty("pageSize", 16384);
        jsonBody.addProperty("maxOrder", 11);
        jsonBody.addProperty("smallCacheSize", 256);
        jsonBody.addProperty("normalCacheSize", 64);
        jsonBody.addProperty("useCacheForAllThreads", true);
        jsonBody.addProperty("directMemoryCacheAlignment", 0);

        Request request = new Request.Builder()
                .url("https://127.0.0.1:9110/v1/configuration/buffer")
                .post(RequestBody.create(jsonBody.toString().getBytes()))
                .header("Content-Type", "application/json")
                .build();

        try (Response response = CustomOkHttpClient.INSTANCE.newCall(request).execute()) {
            assertNotNull(response.body());
            JsonObject responseJson = JsonParser.parseString(response.body().string()).getAsJsonObject();
            System.out.println(responseJson);
            assertTrue(responseJson.get("Success").getAsBoolean());
        }
    }

    @Order(2)
    @Test
    void getDefaultConfigurationTest() throws IOException {
        BufferConfiguration bufferDefault = BufferConfiguration.DEFAULT;

        Request request = new Request.Builder()
                .url("https://127.0.0.1:9110/v1/configuration/buffer/default")
                .get()
                .build();

        try (Response response = CustomOkHttpClient.INSTANCE.newCall(request).execute()) {
            assertNotNull(response.body());
            JsonObject responseJson = JsonParser.parseString(response.body().string()).getAsJsonObject();
            assertTrue(responseJson.get("Success").getAsBoolean());

            JsonObject bufferObject = responseJson.get("Result").getAsJsonObject().get("BufferConfiguration").getAsJsonObject();

            assertEquals(bufferDefault.preferDirect(), bufferObject.get("preferDirect").getAsBoolean());
            assertEquals(bufferDefault.heapArena(), bufferObject.get("heapArena").getAsInt());
            assertEquals(bufferDefault.directArena(), bufferObject.get("directArena").getAsInt());
            assertEquals(bufferDefault.pageSize(), bufferObject.get("pageSize").getAsInt());
            assertEquals(bufferDefault.maxOrder(), bufferObject.get("maxOrder").getAsInt());
            assertEquals(bufferDefault.smallCacheSize(), bufferObject.get("smallCacheSize").getAsInt());
            assertEquals(bufferDefault.useCacheForAllThreads(), bufferObject.get("useCacheForAllThreads").getAsBoolean());
            assertEquals(bufferDefault.directMemoryCacheAlignment(), bufferObject.get("directMemoryCacheAlignment").getAsInt());
        }
    }

    @Order(3)
    @Test
    void getConfigurationTest() throws IOException {
        JsonObject jsonBody = new JsonObject();
        jsonBody.addProperty("preferDirect", true);
        jsonBody.addProperty("heapArena", 256);
        jsonBody.addProperty("directArena", 512);
        jsonBody.addProperty("pageSize", 16384);
        jsonBody.addProperty("maxOrder", 66);
        jsonBody.addProperty("smallCacheSize", 512);
        jsonBody.addProperty("normalCacheSize", 128);
        jsonBody.addProperty("useCacheForAllThreads", true);
        jsonBody.addProperty("directMemoryCacheAlignment", 0);

        Request request = new Request.Builder()
                .url("https://127.0.0.1:9110/v1/configuration/buffer")
                .post(RequestBody.create(jsonBody.toString().getBytes()))
                .header("Content-Type", "application/json")
                .build();

        String id;
        try (Response response = CustomOkHttpClient.INSTANCE.newCall(request).execute()) {
            assertNotNull(response.body());
            JsonObject responseJson = JsonParser.parseString(response.body().string()).getAsJsonObject();
            assertTrue(responseJson.get("Success").getAsBoolean());
        }

        request = new Request.Builder()
                .url("https://127.0.0.1:9110/v1/configuration/buffer")
                .get()
                .build();

        try (Response response = CustomOkHttpClient.INSTANCE.newCall(request).execute()) {
            assertNotNull(response.body());
            JsonObject responseJson = JsonParser.parseString(response.body().string()).getAsJsonObject();
            assertTrue(responseJson.get("Success").getAsBoolean());
            JsonObject bufferObject = responseJson.get("Result").getAsJsonObject().get("BufferConfiguration").getAsJsonObject();

            assertEquals(jsonBody.get("preferDirect").getAsBoolean(), bufferObject.get("preferDirect").getAsBoolean());
            assertEquals(jsonBody.get("heapArena").getAsInt(), bufferObject.get("heapArena").getAsInt());
            assertEquals(jsonBody.get("directArena").getAsInt(), bufferObject.get("directArena").getAsInt());
            assertEquals(jsonBody.get("pageSize").getAsInt(), bufferObject.get("pageSize").getAsInt());
            assertEquals(jsonBody.get("maxOrder").getAsInt(), bufferObject.get("maxOrder").getAsInt());
            assertEquals(jsonBody.get("smallCacheSize").getAsInt(), bufferObject.get("smallCacheSize").getAsInt());
            assertEquals(jsonBody.get("useCacheForAllThreads").getAsBoolean(), bufferObject.get("useCacheForAllThreads").getAsBoolean());
            assertEquals(jsonBody.get("directMemoryCacheAlignment").getAsInt(), bufferObject.get("directMemoryCacheAlignment").getAsInt());
        }
    }
}
