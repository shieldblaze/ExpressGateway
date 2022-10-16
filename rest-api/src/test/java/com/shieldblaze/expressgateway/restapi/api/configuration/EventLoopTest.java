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
import com.shieldblaze.expressgateway.common.curator.Curator;
import com.shieldblaze.expressgateway.configuration.eventloop.EventLoopConfiguration;
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
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

@TestMethodOrder(MethodOrderer.OrderAnnotation.class)
class EventLoopTest {

    @BeforeAll
    static void startSpring() throws IOException {
        ExpressGateway expressGateway = ExpressGatewayConfigured.forTest();
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
    void applyConfiguration() throws IOException {
        JsonObject jsonBody = new JsonObject();
        jsonBody.addProperty("parentWorkers", 2);
        jsonBody.addProperty("childWorkers", 4);

        Request request = new Request.Builder()
                .url("https://127.0.0.1:9110/v1/configuration/eventloop")
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
        JsonObject jsonBody = new JsonObject();
        jsonBody.addProperty("parentWorkers", -2);
        jsonBody.addProperty("childWorkers", -4);

        Request request = new Request.Builder()
                .url("https://127.0.0.1:9110/v1/configuration/eventloop")
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
        EventLoopConfiguration eventLoopDefault = EventLoopConfiguration.DEFAULT;

        Request request = new Request.Builder()
                .url("https://127.0.0.1:9110/v1/configuration/eventloop/default")
                .get()
                .build();

        try (Response response = CustomOkHttpClient.INSTANCE.newCall(request).execute()) {
            assertNotNull(response.body());
            JsonObject responseJson = JsonParser.parseString(response.body().string()).getAsJsonObject();
            assertTrue(responseJson.get("Success").getAsBoolean());
            JsonObject bufferObject = responseJson.get("Result").getAsJsonObject().get("EventLoopConfiguration").getAsJsonObject();

            assertEquals(eventLoopDefault.parentWorkers(), bufferObject.get("parentWorkers").getAsInt());
            assertEquals(eventLoopDefault.childWorkers(), bufferObject.get("childWorkers").getAsInt());
        }
    }

    @Order(4)
    @Test
    void getConfiguration() throws IOException {
        JsonObject jsonBody = new JsonObject();
        jsonBody.addProperty("parentWorkers", 128);
        jsonBody.addProperty("childWorkers", 256);

        Request request = new Request.Builder()
                .url("https://127.0.0.1:9110/v1/configuration/eventloop")
                .post(RequestBody.create(jsonBody.toString().getBytes()))
                .header("Content-Type", "application/json")
                .build();

        try (Response response = CustomOkHttpClient.INSTANCE.newCall(request).execute()) {
            assertNotNull(response.body());
            JsonObject responseJson = JsonParser.parseString(response.body().string()).getAsJsonObject();
            assertTrue(responseJson.get("Success").getAsBoolean());
        }

        request = new Request.Builder()
                .url("https://127.0.0.1:9110/v1/configuration/eventloop")
                .get()
                .build();

        try (Response response = CustomOkHttpClient.INSTANCE.newCall(request).execute()) {
            assertNotNull(response.body());
            JsonObject responseJson = JsonParser.parseString(response.body().string()).getAsJsonObject();
            assertTrue(responseJson.get("Success").getAsBoolean());
            JsonObject bufferObject = responseJson.get("Result").getAsJsonObject().get("EventLoopConfiguration").getAsJsonObject();

            assertEquals(jsonBody.get("parentWorkers").getAsBoolean(), bufferObject.get("parentWorkers").getAsBoolean());
            assertEquals(jsonBody.get("childWorkers").getAsInt(), bufferObject.get("childWorkers").getAsInt());
        }
    }
}
