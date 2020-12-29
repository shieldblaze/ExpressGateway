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
package com.shieldblaze.expressgateway.restapi.config;

import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.MethodOrderer;
import org.junit.jupiter.api.Order;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.TestMethodOrder;
import org.springframework.boot.SpringApplication;
import org.springframework.context.ConfigurableApplicationContext;

import java.io.IOException;
import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

@TestMethodOrder(MethodOrderer.OrderAnnotation.class)
class PooledByteBufAllocatorHandlerTransformerTest {

    final static HttpClient HTTP_CLIENT = HttpClient.newHttpClient();
    static ConfigurableApplicationContext ctx;

    @BeforeAll
    static void setup() {
        System.setProperty("egw.config.dir", System.getProperty("java.io.tmpdir"));
        ctx = SpringApplication.run(Server.class);
    }

    @AfterAll
    static void teardown() {
        ctx.close();
    }

    @Test
    @Order(1)
    void create() throws IOException, InterruptedException {
        JsonObject configJson = new JsonObject();
        configJson.addProperty("preferDirect", true);
        configJson.addProperty("heapArena", 12);
        configJson.addProperty("directArena", 12);
        configJson.addProperty("pageSize", 16384);
        configJson.addProperty("maxOrder", 11);
        configJson.addProperty("smallCacheSize", 256);
        configJson.addProperty("normalCacheSize", 64);
        configJson.addProperty("useCacheForAllThreads", true);
        configJson.addProperty("directMemoryCacheAlignment", 0);

        HttpRequest httpRequest = HttpRequest.newBuilder()
                .POST(HttpRequest.BodyPublishers.ofString(configJson.toString()))
                .uri(URI.create("http://127.0.0.1:9110/config/buffer"))
                .build();

        HttpResponse<String> httpResponse = HTTP_CLIENT.send(httpRequest, HttpResponse.BodyHandlers.ofString());
        assertEquals(204, httpResponse.statusCode());
    }

    @Test
    @Order(2)
    void get() throws IOException, InterruptedException {
        HttpRequest httpRequest = HttpRequest.newBuilder()
                .GET()
                .uri(URI.create("http://127.0.0.1:9110/config/buffer"))
                .build();

        HttpResponse<String> httpResponse = HTTP_CLIENT.send(httpRequest, HttpResponse.BodyHandlers.ofString());
        assertEquals(200, httpResponse.statusCode());

        JsonObject jsonObject = JsonParser.parseString(httpResponse.body()).getAsJsonObject();
        assertTrue(jsonObject.get("preferDirect").getAsBoolean());
        assertEquals(12, jsonObject.get("heapArena").getAsInt());
        assertEquals(12, jsonObject.get("directArena").getAsInt());
        assertEquals(16384, jsonObject.get("pageSize").getAsInt());
        assertEquals(11, jsonObject.get("maxOrder").getAsInt());
        assertEquals(256, jsonObject.get("smallCacheSize").getAsInt());
        assertEquals(64, jsonObject.get("normalCacheSize").getAsInt());
        assertTrue(jsonObject.get("useCacheForAllThreads").getAsBoolean());
        assertEquals(0, jsonObject.get("directMemoryCacheAlignment").getAsInt());
    }
}
