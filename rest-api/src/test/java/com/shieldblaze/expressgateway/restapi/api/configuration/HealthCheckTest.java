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

package com.shieldblaze.expressgateway.restapi.api.configuration;

import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import com.shieldblaze.expressgateway.configuration.healthcheck.HealthCheckConfiguration;
import com.shieldblaze.expressgateway.restapi.RestAPI;
import okhttp3.OkHttpClient;
import okhttp3.Request;
import okhttp3.RequestBody;
import okhttp3.Response;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

import java.io.IOException;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

class HealthCheckTest {

    private static OkHttpClient okHttpClient;

    @BeforeAll
    static void startSpring() {
        RestAPI.start();
        okHttpClient = new OkHttpClient();
        System.setProperty("egw.dir", System.getProperty("java.io.tmpdir"));
    }

    @AfterAll
    static void teardown() {
        okHttpClient.dispatcher().cancelAll();
        RestAPI.stop();
    }

    @Test
    void applyConfiguration() throws IOException {
        JsonObject jsonBody = new JsonObject();
        jsonBody.addProperty("workers", 64);
        jsonBody.addProperty("timeInterval", 1000 * 60);

        Request request = new Request.Builder()
                .url("http://127.0.0.1:9110/v1/configuration/healthcheck/")
                .post(RequestBody.create(jsonBody.toString().getBytes()))
                .header("Content-Type", "application/json")
                .build();

        try (Response response = okHttpClient.newCall(request).execute()) {
            assertNotNull(response.body());
            JsonObject responseJson = JsonParser.parseString(response.body().string()).getAsJsonObject();
            assertTrue(responseJson.get("Success").getAsBoolean());
        }
    }

    @Test
    void applyBadConfiguration() throws IOException {
        JsonObject jsonBody = new JsonObject();
        jsonBody.addProperty("workers", -2);
        jsonBody.addProperty("timeInterval", 1000 * 10);

        Request request = new Request.Builder()
                .url("http://127.0.0.1:9110/v1/configuration/healthcheck/")
                .post(RequestBody.create(jsonBody.toString().getBytes()))
                .header("Content-Type", "application/json")
                .build();

        try (Response response = okHttpClient.newCall(request).execute()) {
            assertNotNull(response.body());
            JsonObject responseJson = JsonParser.parseString(response.body().string()).getAsJsonObject();
            assertFalse(responseJson.get("Success").getAsBoolean());
        }
    }

    @Test
    void getDefaultConfiguration() throws IOException {
        HealthCheckConfiguration healthCheckDefault = HealthCheckConfiguration.DEFAULT;

        Request request = new Request.Builder()
                .url("http://127.0.0.1:9110/v1/configuration/healthcheck/default")
                .get()
                .build();

        try (Response response = okHttpClient.newCall(request).execute()) {
            assertNotNull(response.body());
            JsonObject responseJson = JsonParser.parseString(response.body().string()).getAsJsonObject();
            assertTrue(responseJson.get("Success").getAsBoolean());
            JsonObject bufferObject = responseJson.get("Result").getAsJsonObject().get("HealthCheckConfiguration").getAsJsonObject();

            assertEquals(healthCheckDefault.workers(), bufferObject.get("workers").getAsInt());
            assertEquals(healthCheckDefault.timeInterval(), bufferObject.get("timeInterval").getAsInt());
        }
    }

    @Test
    void getConfiguration() throws IOException {
        JsonObject jsonBody = new JsonObject();
        jsonBody.addProperty("workers", 128);
        jsonBody.addProperty("timeInterval", 1000 * 60);

        Request request = new Request.Builder()
                .url("http://127.0.0.1:9110/v1/configuration/healthcheck/")
                .post(RequestBody.create(jsonBody.toString().getBytes()))
                .header("Content-Type", "application/json")
                .build();

        try (Response response = okHttpClient.newCall(request).execute()) {
            assertNotNull(response.body());
            JsonObject responseJson = JsonParser.parseString(response.body().string()).getAsJsonObject();
            assertTrue(responseJson.get("Success").getAsBoolean());
        }

        request = new Request.Builder()
                .url("http://127.0.0.1:9110/v1/configuration/healthcheck/")
                .get()
                .build();

        try (Response response = okHttpClient.newCall(request).execute()) {
            assertNotNull(response.body());
            JsonObject responseJson = JsonParser.parseString(response.body().string()).getAsJsonObject();
            assertTrue(responseJson.get("Success").getAsBoolean());
            JsonObject bufferObject = responseJson.get("Result").getAsJsonObject().get("HealthCheckConfiguration").getAsJsonObject();

            assertEquals(jsonBody.get("workers").getAsInt(), bufferObject.get("workers").getAsInt());
            assertEquals(jsonBody.get("timeInterval").getAsInt(), bufferObject.get("timeInterval").getAsInt());
        }
    }
}
